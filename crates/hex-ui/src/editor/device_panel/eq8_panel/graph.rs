//! EQ response sampling, graph interaction, and parameter mapping.
use super::*;

pub(super) fn graph_entity(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    owner: hex_engine::ProcessorChainOwner,
    model: ResponseGraphModel,
) -> Stateful<Div> {
    view.device_panel
        .eq8_graphs
        .entry(device)
        .or_insert_with(|| {
            let style = theme::device_style(theme::DeviceStyleRole::Display);
            let entity = cx.new(|graph_cx| {
                let mut graph =
                    ResponseGraph::new(graph_cx, format!("eq8-response-{}", device.value()), style);
                graph.set_editable(true, graph_cx);
                graph.set_marquee_enabled(true, graph_cx);
                graph.set_backspace_removal(true);
                graph
            });
            let generation = view.project_generation;
            let subscription =
                cx.subscribe(&entity, move |view, _, event: &ResponseGraphEvent, cx| {
                    if view.project_generation == generation {
                        handle_graph_event(view, device, owner, *event, cx);
                    }
                });
            let marquee = cx.subscribe(
                &entity,
                move |view, _, event: &hex_widgets::ResponseGraphMarqueeEvent, cx| {
                    if view.project_generation == generation && finalize_controls(view, cx) {
                        if let Some(panel) = view.device_panel.eq8_panels.get_mut(&device) {
                            selection::marquee_event(panel, event);
                        }
                        cx.notify();
                    }
                },
            );
            GraphControl {
                entity,
                _subscription: subscription,
                _marquee_subscription: marquee,
                group_drag: None,
            }
        });
    let Some(graph) = view.device_panel.eq8_graphs.get(&device) else {
        return div()
            .id(ElementId::Name(
                format!("eq8-response-unavailable-{}", device.value()).into(),
            ))
            .child(unavailable("response graph"));
    };
    let graph_entity = graph.entity.clone();
    graph_entity.update(cx, |graph, cx| {
        let _ = graph.set_model(
            model,
            theme::device_style(theme::DeviceStyleRole::Display),
            cx,
        );
    });
    div()
        .id(ElementId::Name(
            format!("eq8-graph-slot-{}", device.value()).into(),
        ))
        .debug_selector(move || format!("eq8-graph-slot-{}", device.value()))
        .w_full()
        .h_full()
        .min_h_0()
        .child(graph_entity)
}

fn handle_graph_event(
    view: &mut EditorView,
    device: DeviceId,
    owner: hex_engine::ProcessorChainOwner,
    event: ResponseGraphEvent,
    cx: &mut Context<EditorView>,
) {
    let band = event.node_id as usize;
    if band >= BAND_COUNT {
        return;
    }
    if event.phase == hex_widgets::ResponseGraphPhase::Select {
        if !finalize_controls(view, cx) {
            return;
        }
        if let Some(panel) = view.device_panel.eq8_panels.get_mut(&device) {
            selection::select(panel, band, event.modifiers.shift);
        }
        cx.notify();
        return;
    }
    if event.phase == hex_widgets::ResponseGraphPhase::Remove {
        group::disable(view, device, cx);
        return;
    }
    if event.phase == hex_widgets::ResponseGraphPhase::Press {
        begin_band_audition(view, device, owner, band);
        return;
    }
    if event.phase == hex_widgets::ResponseGraphPhase::Release {
        if let Some(panel) = view.device_panel.eq8_panels.get_mut(&device) {
            selection::release(panel, band);
        }
        if view
            .device_panel
            .eq8_panels
            .get(&device)
            .is_some_and(|panel| panel.audition_gesture_active)
        {
            if let Some(audio) = view.audio.as_mut() {
                let _ = audio.control.audition_release();
            }
            if let Some(panel) = view.device_panel.eq8_panels.get_mut(&device) {
                panel.audition_gesture_active = false;
            }
        }
        return;
    }
    if matches!(
        event.phase,
        hex_widgets::ResponseGraphPhase::Cancel | hex_widgets::ResponseGraphPhase::ReleaseOutside
    ) && view
        .device_panel
        .eq8_panels
        .get(&device)
        .is_some_and(|panel| panel.audition_gesture_active)
    {
        if let Some(audio) = view.audio.as_mut() {
            let _ = audio.control.audition_cancel();
        }
        if let Some(panel) = view.device_panel.eq8_panels.get_mut(&device) {
            panel.audition_gesture_active = false;
        }
    }
    // Physical release never edits the authored parameter preview. A Commit
    // may still be awaiting the worker-backed gesture barrier.
    if event.phase == hex_widgets::ResponseGraphPhase::ReleaseOutside {
        return;
    }
    let selected = view
        .device_panel
        .eq8_panels
        .get(&device)
        .map_or(0, selection::mask);
    let group_active = view
        .device_panel
        .eq8_graphs
        .get(&device)
        .is_some_and(|graph| graph.group_drag.is_some());
    if (event.phase == hex_widgets::ResponseGraphPhase::Begin
        && selected.count_ones() > 1
        && selected & (1 << band) != 0)
        || group_active
    {
        if let Some(panel) = view.device_panel.eq8_panels.get_mut(&device) {
            panel.collapse_band = None;
        }
        group::event(view, device, selected, event);
        cx.notify();
        return;
    }
    if event.phase == hex_widgets::ResponseGraphPhase::Begin {
        if let Some(panel) = view.device_panel.eq8_panels.get_mut(&device) {
            panel.collapse_band = None;
        }
        if view.device_gesture.snapshot().is_some() {
            view.device_panel.message = Some("wait for the current device edit, then retry".into());
            return;
        }
    }
    let Some(snapshot) = view.device_gesture.snapshot() else {
        if event.phase != hex_widgets::ResponseGraphPhase::Begin {
            return;
        }
        let Some(device_view) = view
            .device_panel
            .views
            .get(&device)
            .and_then(|(_, _, view)| view.as_ref().ok())
            .cloned()
        else {
            return;
        };
        let mode = value(view, device, &device_view, id(201))
            .and_then(choice)
            .unwrap_or(0)
            .min(2);
        let lane = view
            .device_panel
            .eq8_panels
            .get(&device)
            .map_or(0, |panel| panel.selected_lane)
            .min(if mode == 0 { 0 } else { 1 });
        let bank = match mode {
            1 => 1 + lane,
            2 => 3 + lane,
            _ => 0,
        };
        let second = if band_kind(view, device, &device_view, bank, band)
            .is_some_and(|kind| kind >= 3)
            || event.modifiers.alt
        {
            band_id(bank, band, 4)
        } else {
            band_id(bank, band, 3)
        };
        let Some(ParameterValue::Float(frequency)) =
            value(view, device, &device_view, band_id(bank, band, 2))
        else {
            return;
        };
        let Some(ParameterValue::Float(secondary)) = value(view, device, &device_view, second)
        else {
            return;
        };
        let q_by_delta = second == band_id(bank, band, 4)
            && band_kind(view, device, &device_view, bank, band).is_some_and(|kind| kind < 3);
        if let Err(error) = view.begin_device_pair_gesture(
            device,
            band_id(bank, band, 2),
            second,
            ParameterPairValue {
                first: ParameterValue::Float(frequency),
                second: ParameterValue::Float(secondary),
            },
        ) {
            view.device_panel.message = Some(error.to_owned());
            return;
        }
        view.device_panel.pending_values.insert(
            ParameterKey {
                device,
                parameter: band_id(bank, band, 2),
            },
            ParameterValue::Float(frequency),
        );
        view.device_panel.pending_values.insert(
            ParameterKey {
                device,
                parameter: second,
            },
            ParameterValue::Float(secondary),
        );
        view.device_panel.eq8_graph_gestures.insert(
            device,
            GraphGestureState {
                origin_y: event.position[1],
                initial_q: if second == band_id(bank, band, 4) {
                    secondary
                } else {
                    1.0
                },
                q_by_delta,
            },
        );
        cx.notify();
        return;
    };
    let GestureTarget::DeviceParameterPair {
        device: target_device,
        first,
        second,
    } = snapshot.target
    else {
        return;
    };
    if target_device != device {
        return;
    }
    if event.phase == hex_widgets::ResponseGraphPhase::Cancel {
        view.device_panel.pending_values.remove(&ParameterKey {
            device,
            parameter: first,
        });
        view.device_panel.pending_values.remove(&ParameterKey {
            device,
            parameter: second,
        });
        let _ = view.cancel_device_gesture();
        view.device_panel.eq8_graph_gestures.remove(&device);
        cx.notify();
        return;
    }
    let second_value = if second.value() == first.value() + 2 {
        let q = view
            .device_panel
            .eq8_graph_gestures
            .get(&device)
            .map_or_else(
                || q_from_y(event.position[1]),
                |origin| {
                    if origin.q_by_delta {
                        (origin.initial_q
                            * ((MAX_Q / MIN_Q).ln() * (event.position[1] - origin.origin_y)).exp())
                        .clamp(MIN_Q, MAX_Q)
                    } else {
                        q_from_y(event.position[1])
                    }
                },
            );
        ParameterValue::Float(q)
    } else {
        ParameterValue::Float(gain_from_y(event.position[1]))
    };
    let pair = ParameterPairValue {
        first: ParameterValue::Float(frequency_from_x(
            event.position[0],
            view.state
                .as_ref()
                .map_or(48_000, |state| state.project.sample_rate()),
        )),
        second: second_value,
    };
    view.device_panel.pending_values.insert(
        ParameterKey {
            device,
            parameter: first,
        },
        pair.first,
    );
    view.device_panel.pending_values.insert(
        ParameterKey {
            device,
            parameter: second,
        },
        pair.second,
    );
    match event.phase {
        hex_widgets::ResponseGraphPhase::Change => {
            if let Err(error) = view.change_device_pair_gesture(pair) {
                view.device_panel.message = Some(error.to_owned());
            }
        }
        hex_widgets::ResponseGraphPhase::Commit => {
            if let Err(error) = view.finish_device_pair_gesture(pair) {
                view.device_panel.message = Some(error.to_owned());
            }
            view.device_panel.eq8_graph_gestures.remove(&device);
        }
        _ => {}
    }
    cx.notify();
}

fn begin_band_audition(
    view: &mut EditorView,
    device: DeviceId,
    owner: hex_engine::ProcessorChainOwner,
    band: usize,
) {
    if view.document_audio_mode == hex_session::DocumentAudioMode::VerifiedStreaming {
        view.device_panel.message =
            Some("eq8 band audition is unavailable for verified streaming audio".into());
        return;
    }
    if !view
        .device_panel
        .eq8_panels
        .get(&device)
        .is_some_and(|panel| panel.audition_armed)
    {
        return;
    }
    let Some(device_view) = view
        .device_panel
        .views
        .get(&device)
        .and_then(|(_, _, view)| view.as_ref().ok())
        .cloned()
    else {
        return;
    };
    let mode = value(view, device, &device_view, id(201))
        .and_then(choice)
        .unwrap_or(0)
        .min(2);
    let lane = view
        .device_panel
        .eq8_panels
        .get(&device)
        .map_or(0, |panel| panel.selected_lane)
        .min(if mode == 0 { 0 } else { 1 });
    let generation = view.project_generation;
    let project_revision = view.state.as_ref().map_or(0, |state| state.revision);
    let Some(floor) = view
        .audio_admission
        .published_audio_revision(generation, project_revision)
    else {
        view.device_panel.message =
            Some("wait for the current eq8 revision before auditioning".into());
        return;
    };
    let Some(audio) = view.audio.as_mut() else {
        view.device_panel.message = Some("audio output is unavailable".into());
        return;
    };
    let identity = hex_engine::PostNodeIdentity::new(owner, device.value(), device_view.type_id);
    let selection = hex_engine::ProcessorAuditionSelection {
        index: band as u32,
        lane: lane as u8,
    };
    let target = hex_audio::processor_audition::ProcessorAuditionTarget {
        identity,
        project_generation: generation,
        activation_floor: floor,
        sample_rate_hz: audio.sample_rate,
        selection,
    };
    match audio.control.audition_select(target) {
        Ok(_) => {
            if let Some(panel) = view.device_panel.eq8_panels.get_mut(&device) {
                panel.audition_gesture_active = true;
            }
            view.device_panel.message = None;
        }
        Err(_) => {
            view.device_panel.message =
                Some("eq8 band audition is unavailable for this target".into());
        }
    }
}

pub(super) struct GraphModelRequest<'a> {
    pub(super) device: DeviceId,
    pub(super) owner: hex_engine::ProcessorChainOwner,
    pub(super) canonical_state: &'a NativeDeviceState,
    pub(super) device_view: &'a NativeDeviceView,
    pub(super) sample_rate: u32,
    pub(super) bank: usize,
    pub(super) lane: usize,
    pub(super) selected_band: usize,
}

pub(super) fn graph_model(
    view: &mut EditorView,
    request: GraphModelRequest<'_>,
) -> Option<ResponseGraphModel> {
    let GraphModelRequest {
        device,
        owner,
        canonical_state,
        device_view,
        sample_rate,
        bank,
        lane,
        selected_band,
    } = request;
    let display_state = effective_state(view, device, canonical_state, device_view)?;
    let cache_matches = view
        .device_panel
        .eq8_responses
        .get(&device)
        .is_some_and(|cache| cache.state == display_state && cache.sample_rate == sample_rate);
    if !cache_matches {
        let upper = MAX_HZ.min(sample_rate as f32 * 0.49);
        if upper <= MIN_HZ {
            return None;
        }
        let frequencies = logarithmic_frequencies(MIN_HZ, upper, RESPONSE_POINTS)?;
        let lanes =
            hex_engine::native_effect_frequency_response(&display_state, sample_rate, &frequencies)
                .ok()??;
        view.device_panel.eq8_responses.insert(
            device,
            ResponseCache {
                state: display_state,
                sample_rate,
                lanes,
            },
        );
    }
    let response = view.device_panel.eq8_responses.get(&device)?.clone();
    let primary = response
        .lanes
        .iter()
        .enumerate()
        .map(|(index, lanes)| {
            [
                index as f32 / (response.lanes.len() - 1) as f32,
                magnitude_to_y(lanes[lane]),
            ]
        })
        .collect();
    let other_lane = if lane == 0 { 1 } else { 0 };
    let secondary = response
        .lanes
        .iter()
        .enumerate()
        .map(|(index, lanes)| {
            [
                index as f32 / (response.lanes.len() - 1) as f32,
                magnitude_to_y(lanes[other_lane]),
            ]
        })
        .collect();
    let sample_rate_max = (sample_rate as f32 * 0.49).min(MAX_HZ);
    let nodes = (0..BAND_COUNT)
        .map(|band| {
            let frequency = value(view, device, device_view, band_id(bank, band, 2))
                .and_then(float)
                .unwrap_or(MIN_HZ);
            let gain = value(view, device, device_view, band_id(bank, band, 3))
                .and_then(float)
                .unwrap_or(0.0);
            let enabled = value(view, device, device_view, band_id(bank, band, 0))
                .and_then(boolean)
                .unwrap_or(false);
            let kind = value(view, device, device_view, band_id(bank, band, 1))
                .and_then(choice)
                .unwrap_or(0);
            let y = if kind >= 3 {
                value(view, device, device_view, band_id(bank, band, 4))
                    .and_then(float)
                    .map(|q| log_position(q, MIN_Q, MAX_Q))
                    .unwrap_or(0.5)
            } else {
                ((gain - MIN_GAIN_DB) / (MAX_GAIN_DB - MIN_GAIN_DB)).clamp(0.0, 1.0)
            };
            ResponseGraphNode {
                id: band as u32,
                position: [log_position(frequency, MIN_HZ, sample_rate_max), y],
                enabled,
                selected: view.device_panel.eq8_panels.get(&device).map_or_else(
                    || band == selected_band,
                    |panel| selection::mask(panel) & (1 << band) != 0,
                ),
            }
        })
        .collect();
    let target_identity =
        hex_engine::PostNodeIdentity::new(owner, device.value(), device_view.type_id);
    let output = super::super::eq_panel::spectrum_result(view, target_identity).map(|result| {
        let frame = &result.output.frame;
        let upper = MAX_HZ.min(frame.sample_rate_hz as f32 * 0.49);
        super::super::spectrum_display::curves(frame, |hz| log_position(hz, MIN_HZ, upper))
    });
    let (analyzer, analyzer_peak) = output.map_or((None, None), |curves| {
        (Some(curves.level), Some(curves.peak))
    });
    Some(ResponseGraphModel {
        primary,
        secondary,
        analyzer,
        input_analyzer: None,
        analyzer_peak,
        nodes,
    })
}

fn effective_state(
    view: &mut EditorView,
    device: DeviceId,
    canonical_state: &NativeDeviceState,
    device_view: &NativeDeviceView,
) -> Option<NativeDeviceState> {
    let mut state = canonical_state.clone();
    for parameter in &device_view.parameters {
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
                NativeDeviceClass::Effect,
                &state,
                parameter.descriptor.id,
                effective,
            )
            .ok()?;
        }
    }
    Some(state)
}

fn band_kind(
    view: &mut EditorView,
    device: DeviceId,
    device_view: &NativeDeviceView,
    bank: usize,
    band: usize,
) -> Option<usize> {
    value(view, device, device_view, band_id(bank, band, 1)).and_then(choice)
}

pub(super) fn band_id(bank: usize, band: usize, field: u32) -> ParameterId {
    ParameterId::from_static((bank as u32) * 40 + (band as u32) * 5 + field + 1)
}

pub(super) const fn id(value: u32) -> ParameterId {
    ParameterId::from_static(value)
}

pub(super) fn value(
    view: &mut EditorView,
    device: DeviceId,
    device_view: &NativeDeviceView,
    parameter_id: ParameterId,
) -> Option<ParameterValue> {
    let parameter = parameter(device_view.parameters.as_slice(), parameter_id)?;
    Some(effective_parameter_value(
        view,
        ParameterKey {
            device,
            parameter: parameter_id,
        },
        parameter.value,
    ))
}

pub(super) fn parameter(
    parameters: &[NativeDeviceParameterView],
    id: ParameterId,
) -> Option<&NativeDeviceParameterView> {
    parameters
        .iter()
        .find(|parameter| parameter.descriptor.id == id)
}

pub(super) fn logarithmic_frequencies(min: f32, max: f32, count: usize) -> Option<Vec<f32>> {
    if !min.is_finite()
        || !max.is_finite()
        || min <= 0.0
        || max <= min
        || !(2..=256).contains(&count)
    {
        return None;
    }
    let mut output = Vec::new();
    output.try_reserve_exact(count).ok()?;
    let ratio = (max / min).ln();
    for index in 0..count {
        output.push(min * (ratio * index as f32 / (count - 1) as f32).exp());
    }
    Some(output)
}

pub(super) fn log_position(value: f32, min: f32, max: f32) -> f32 {
    if value <= min {
        return 0.0;
    }
    if value >= max {
        return 1.0;
    }
    ((value / min).ln() / (max / min).ln()).clamp(0.0, 1.0)
}

pub(super) fn frequency_from_x(x: f32, sample_rate: u32) -> f32 {
    let max = MAX_HZ.min(sample_rate as f32 * 0.49);
    MIN_HZ * ((max / MIN_HZ).ln() * x.clamp(0.0, 1.0)).exp()
}

pub(super) fn gain_from_y(y: f32) -> f32 {
    MIN_GAIN_DB + (MAX_GAIN_DB - MIN_GAIN_DB) * y.clamp(0.0, 1.0)
}

pub(super) fn q_from_y(y: f32) -> f32 {
    MIN_Q * ((MAX_Q / MIN_Q).ln() * y.clamp(0.0, 1.0)).exp()
}

fn magnitude_to_y(magnitude: f32) -> f32 {
    let db = if magnitude > 0.0 {
        20.0 * magnitude.log10()
    } else {
        MIN_GAIN_DB
    };
    ((db - MIN_GAIN_DB) / (MAX_GAIN_DB - MIN_GAIN_DB)).clamp(0.0, 1.0)
}

pub(super) fn float(value: ParameterValue) -> Option<f32> {
    match value {
        ParameterValue::Float(value) if value.is_finite() => Some(value),
        _ => None,
    }
}

pub(super) fn choice(value: ParameterValue) -> Option<usize> {
    match value {
        ParameterValue::Choice(value) => usize::try_from(value).ok(),
        _ => None,
    }
}

fn boolean(value: ParameterValue) -> Option<bool> {
    match value {
        ParameterValue::Bool(value) => Some(value),
        _ => None,
    }
}

#[cfg(all(test, not(feature = "ui-test-support")))]
mod tests {
    use super::*;

    #[test]
    fn graph_axis_and_dial_mappings_use_logarithmic_domains() {
        assert!((frequency_from_x(0.0, 48_000) - 20.0).abs() < 0.001);
        assert!((frequency_from_x(1.0, 48_000) - 20_000.0).abs() < 0.1);
        assert!((frequency_from_x(1.0, 8_000) - 3_920.0).abs() < 0.1);
        assert!((log_position(1_000.0, 20.0, 20_000.0) - 0.5663).abs() < 0.001);
        assert!((q_from_y(0.0) - MIN_Q).abs() < 0.001);
        assert!((q_from_y(1.0) - MAX_Q).abs() < 0.001);
        assert_eq!(gain_from_y(0.0), MIN_GAIN_DB);
        assert_eq!(gain_from_y(1.0), MAX_GAIN_DB);
    }

    #[test]
    fn response_sampling_is_bounded_and_strictly_increasing() {
        let frequencies = logarithmic_frequencies(20.0, 20_000.0, RESPONSE_POINTS).unwrap();
        assert_eq!(frequencies.len(), RESPONSE_POINTS);
        assert!(frequencies.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(logarithmic_frequencies(0.0, 20_000.0, RESPONSE_POINTS).is_none());
        assert!(logarithmic_frequencies(20.0, 20_000.0, 257).is_none());
    }
}
