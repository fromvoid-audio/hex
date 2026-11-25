//! Device chain owner dispatch and track, source, effect, and instrument rendering.
use super::*;

pub(in crate::editor) fn view(
    view: &mut EditorView,
    window: Option<&mut Window>,
    cx: &mut Context<EditorView>,
) -> Stateful<Div> {
    retire_removed_send_controls(view, cx);
    if view.device_panel.generation != view.project_generation {
        // Project replacement retires the worker gesture. Drop old subscriptions
        // before stale controls can send events into the replacement project.
        clear_owner_state(view, cx, true, None);
    }
    reconcile_eq_spectrum(view);
    sampler_overview::reconcile_cursor(view);
    if window
        .as_deref()
        .is_some_and(|window| !view.focus.is_focused(window))
    {
        cancel_sampler_boundary(view);
    }
    let Some(state) = view.state.as_ref().cloned() else {
        return empty_panel("project is still loading");
    };
    let inventory_is_current = view
        .device_panel
        .operator_preview_inventory
        .as_ref()
        .is_some_and(|inventory| {
            inventory.generation == view.project_generation && inventory.revision == state.revision
        });
    if !inventory_is_current {
        view.device_panel
            .collapsed
            .retain(|device| state.project.device(*device).is_some());
        let devices = state
            .project
            .native_sources()
            .iter()
            .filter_map(|source| {
                let device = source.instrument();
                (device.state().type_id().as_str().ok() == Some("hex.instrument.operator"))
                    .then_some(device.id())
            })
            .collect();
        view.device_panel.operator_preview_inventory = Some(OperatorPreviewInventory {
            generation: view.project_generation,
            revision: state.revision,
            devices: Arc::new(devices),
        });
    }
    let alive_operator = view
        .device_panel
        .operator_preview_inventory
        .as_ref()
        .map_or_else(
            || Arc::new(HashSet::new()),
            |inventory| Arc::clone(&inventory.devices),
        );
    operator_panel::preview::retire_removed(view, cx, alive_operator.as_ref());
    if view.selected_track.is_some() || view.session_grid.focused_return.is_some() {
        view.device_panel.main_selected = None;
    }
    if let Some((generation, source, anchor)) = view.device_panel.source_manager.source_view
        && (generation != view.project_generation
            || anchor != effect_bus::selected_track_owner(view, &state)
            || !state
                .project
                .native_source(source)
                .is_some_and(|definition| {
                    matches!(
                        definition.instrument().state().type_id().as_str(),
                        Ok("hex.instrument.fm4"
                            | "hex.instrument.operator"
                            | "hex.instrument.sampler")
                    )
                })
            || state
                .project
                .tracks()
                .iter()
                .any(|track| track.source_id() == Some(source)))
    {
        view.device_panel.source_manager.source_view = None;
    }
    let desired_owner = selected_owner(view, &state);
    if desired_owner != view.device_panel.owner {
        if let Some(audio) = view.audio.as_mut() {
            let _ = audio.control.audition_cancel();
        }
        if let Some(previous) = view.device_panel.owner {
            if !owner_exists(&state, previous) {
                // A history restore or external project replacement can remove
                // an owner while its view is focused. Cancel its drafts before
                // switching; never commit an edit against a stale device ID.
                cancel_all_controls(view, cx);
                if view.device_panel.gesture.snapshot().is_some() {
                    let _ = view.cancel_device_gesture();
                    view.device_panel.message = Some("canceling removed device edit".into());
                    return empty_panel("canceling removed device edit");
                }
            } else if !finalize_controls(view, cx) {
                view.device_panel.message = Some("finish the active device value first".into());
                return render_owner(view, window, cx, &state, previous);
            } else if view.device_panel.gesture.snapshot().is_some() {
                view.device_panel.message = Some("finishing device edit".into());
                return render_owner(view, window, cx, &state, previous);
            }
        }
        if let Some(audio) = view.audio.as_mut() {
            let _ = audio.control.spectrum_clear();
        }
        clear_owner_state(view, cx, false, desired_owner);
    }
    match view.device_panel.owner {
        Some(owner) => render_owner(view, window, cx, &state, owner),
        None => {
            let manager = source_manager::button(view, window.as_deref(), cx);
            empty_panel("select a track or return to view devices")
                .children(std::iter::once(manager))
                .on_drop(crate::editor::sampler_load::track_drop_listener(
                    cx,
                    None,
                    hex_project::InstrumentLoadPlacement::First,
                ))
        }
    }
}

pub(in crate::editor) fn render_owner(
    view: &mut EditorView,
    window: Option<&mut Window>,
    cx: &mut Context<EditorView>,
    state: &ViewState,
    owner: DevicePanelOwner,
) -> Stateful<Div> {
    automation_playback::begin(view);
    match owner {
        DevicePanelOwner::Main => {
            return_view::render(view, window, cx, state, EffectBusOwner::Main).on_drop(
                crate::editor::sampler_load::track_drop_listener(
                    cx,
                    None,
                    hex_project::InstrumentLoadPlacement::First,
                ),
            )
        }
        DevicePanelOwner::Track(track) => render_track(view, window, cx, state, track),
        DevicePanelOwner::Source(source) => {
            render_source(view, window.as_deref(), cx, state, source)
        }
        DevicePanelOwner::Return(return_track) => return_view::render(
            view,
            window,
            cx,
            state,
            EffectBusOwner::Return(return_track),
        )
        .on_drop(crate::editor::sampler_load::track_drop_listener(
            cx,
            None,
            hex_project::InstrumentLoadPlacement::First,
        )),
    }
}

/// Whether a retained source is still the visible, unassigned panel owner.
pub(in crate::editor) fn render_source(
    view: &mut EditorView,
    window: Option<&Window>,
    cx: &mut Context<EditorView>,
    state: &ViewState,
    source: hex_project::SourceId,
) -> Stateful<Div> {
    let Some(definition) = state.project.native_source(source) else {
        return empty_panel("source is no longer available");
    };
    let device = definition.instrument();
    let metadata = match hex_engine::native_device_view_for_class(
        NativeDeviceClass::Instrument,
        device.state(),
    ) {
        Ok(metadata) => metadata,
        Err(_) => {
            let manager = source_manager::button(view, window, cx);
            let close = source_manager::close_button(view, window, cx);
            return empty_panel("unsupported instrument state")
                .children(std::iter::once(manager))
                .children(std::iter::once(close));
        }
    };
    let identity = DeviceControlIdentity {
        class: NativeDeviceClass::Instrument,
        type_id: metadata.type_id,
    };
    automation_playback::sample(view, device, identity.class, Some(&metadata));
    view.device_panel.views.insert(
        device.id(),
        (identity.class, device.state().clone(), Ok(metadata.clone())),
    );
    let sampler_source = metadata.type_id.as_str() == Ok("hex.instrument.sampler");
    let body = if sampler_source {
        if hex_engine::is_empty_sampler_state_on_control(device.state()) {
            div()
                .id("source-sampler-empty")
                .debug_selector(|| "source-sampler-empty".into())
                .p(px(8.0))
                .child(theme::text("empty sampler · drop a browser sample here"))
        } else {
            let owner = hex_session::service::SamplerParameterOwner::Source(source);
            let mut fields = [
                hex_engine::SamplerParameter::Gain,
                hex_engine::SamplerParameter::Transpose,
                hex_engine::SamplerParameter::StartFrame,
                hex_engine::SamplerParameter::EndFrame,
            ]
            .into_iter()
            .map(|parameter| view.sampler_parameter_control(owner, device, parameter, cx))
            .collect::<Vec<_>>();
            let loop_available = hex_engine::sampler_parameter_value_on_control(
                hex_engine::SamplerParameter::LoopEnabled,
                device.state(),
            )
            .is_ok();
            if loop_available {
                for parameter in [
                    hex_engine::SamplerParameter::LoopEnabled,
                    hex_engine::SamplerParameter::LoopStartFrame,
                    hex_engine::SamplerParameter::LoopEndFrame,
                    hex_engine::SamplerParameter::LoopCrossfadeMs,
                ] {
                    fields.push(view.sampler_parameter_control(owner, device, parameter, cx));
                }
            }
            for parameter in [
                hex_engine::SamplerParameter::AttackMs,
                hex_engine::SamplerParameter::DecayMs,
                hex_engine::SamplerParameter::SustainPercent,
                hex_engine::SamplerParameter::ReleaseMs,
            ] {
                fields.push(view.sampler_parameter_control(owner, device, parameter, cx));
            }
            let overview = sampler_overview::render(view, owner, device, "sampler", cx);
            div()
                .id("source-sampler-scroll")
                .debug_selector(|| "source-sampler-scroll".into())
                .flex_1()
                .min_h_0()
                .w_full()
                .overflow_x_scroll()
                .overflow_y_hidden()
                .child(
                    div()
                        .w(px(crate::device_composition::SAMPLER_FRAME_WIDTH))
                        .h(px(DEVICE_PANEL_HEIGHT - DEVICE_PANEL_HEADER_HEIGHT))
                        .flex_shrink_0()
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .p(px(4.0))
                        .child(overview)
                        .child(theme::text("drop a browser sample here to replace"))
                        .child(
                            div()
                                .grid()
                                .grid_cols(if loop_available { 12 } else { 8 })
                                .gap(px(2.0))
                                .children(fields),
                        ),
                )
        }
    } else if metadata.type_id.as_str() == Ok("hex.instrument.operator") {
        let body = operator_panel::render_body(view, cx, device.id(), &metadata);
        let width = operator_panel::frame_width(
            view.device_panel
                .operator_pages
                .get(&device.id())
                .copied()
                .unwrap_or_default(),
        );
        let body = body.w(px(width)).flex_shrink_0();
        div()
            .id("source-operator-scroll")
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_x_scroll()
            .overflow_y_hidden()
            .child(body)
    } else {
        parameter_rows::body(view, window, cx, device.id(), identity, &metadata)
    };
    let manager = source_manager::button(view, window, cx);
    let close = source_manager::close_button(view, window, cx);
    div()
        .id("source-device-panel")
        .debug_selector(|| "source-device-panel".into())
        .h(px(DEVICE_PANEL_HEIGHT))
        .w_full()
        .flex_shrink_0()
        .flex()
        .flex_col()
        .bg(rgb(theme::color(theme::PaletteRole::Surface)))
        .when(sampler_source, |panel| {
            panel.on_drop(
                cx.listener(move |view, drag: &drag::BrowserAudioDrag, _, cx| {
                    view.drop_browser_sample_on_source(drag, source, cx);
                }),
            )
        })
        .child(
            div()
                .h(px(DEVICE_PANEL_HEADER_HEIGHT))
                .flex()
                .items_center()
                .gap(px(4.0))
                .children(std::iter::once(manager))
                .child(theme::text(format!(
                    "source · {}",
                    hex_session::service::text::verbatim(definition.name())
                )))
                .children(std::iter::once(close)),
        )
        .child(body)
}

pub(in crate::editor) fn render_track(
    view: &mut EditorView,
    mut window: Option<&mut Window>,
    cx: &mut Context<EditorView>,
    state: &ViewState,
    track_id: TrackId,
) -> Stateful<Div> {
    let Some(track) = state.project.track(track_id) else {
        return empty_panel("selected track is no longer available");
    };
    let source_effects = track
        .source_id()
        .and_then(|source| state.project.native_source(source))
        .map_or(&[][..], hex_project::NativeSource::post_instrument_effects);
    let is_source_effect = |id| source_effects.iter().any(|effect| effect.id() == id);
    let nodes: Vec<_> = track
        .ordered_device_chain()
        .flat_map(|node| {
            let suffix = if matches!(node, hex_project::TrackDeviceRef::Instrument(_)) {
                source_effects
            } else {
                &[]
            };
            std::iter::once(node).chain(suffix.iter().map(hex_project::TrackDeviceRef::Effect))
        })
        .collect();
    let identities: HashMap<_, _> = track
        .ordered_device_chain()
        .map(|node| match node {
            hex_project::TrackDeviceRef::Instrument(device) => (
                device.id(),
                DeviceControlIdentity {
                    class: NativeDeviceClass::Instrument,
                    type_id: device.state().type_id(),
                },
            ),
            hex_project::TrackDeviceRef::Effect(device) => (
                device.id(),
                DeviceControlIdentity {
                    class: NativeDeviceClass::Effect,
                    type_id: device.state().type_id(),
                },
            ),
        })
        .chain(
            source_effects
                .iter()
                .filter(|device| modulators::source_utility_gain_available(&state.project, device))
                .map(|device| {
                    (
                        device.id(),
                        DeviceControlIdentity {
                            class: NativeDeviceClass::Effect,
                            type_id: device.state().type_id(),
                        },
                    )
                }),
        )
        .collect();
    let alive: HashSet<_> = identities
        .keys()
        .copied()
        .chain(source_effects.iter().map(hex_project::Device::id))
        .collect();
    view.device_panel.reconcile_selection(&alive);

    clear_removed_processor_audition_target(
        view,
        hex_engine::ProcessorChainOwner::Track(track_id.value()),
        &identities,
    );
    view.device_panel
        .meter_states
        .retain(|id, _| identities.contains_key(id));
    if view
        .device_panel
        .hovered_meter
        .is_some_and(|id| !identities.contains_key(&id))
    {
        view.device_panel.hovered_meter = None;
    }
    let stale_keys: Vec<_> = view
        .device_panel
        .controls
        .iter()
        .filter(|(key, control)| {
            identities
                .get(&key.device)
                .is_none_or(|identity| !parameter_control_matches_identity(control, *identity))
        })
        .map(|(key, _)| *key)
        .collect();
    let mut changed_device_ids: HashSet<_> = stale_keys.iter().map(|key| key.device).collect();
    changed_device_ids.extend(view.device_panel.views.iter().filter_map(
        |(device, (class, old_state, _))| {
            identities.get(device).and_then(|identity| {
                (*class != identity.class || old_state.type_id() != identity.type_id)
                    .then_some(*device)
            })
        },
    ));
    let active_target = view
        .device_panel
        .gesture
        .snapshot()
        .map(|snapshot| snapshot.target);
    snapshot::retire_invalid_owner(view, &alive);
    let stale_target_has_work = active_target.is_some_and(|target| match target {
        GestureTarget::DeviceParameter { device, .. } => !alive.contains(&device),
        GestureTarget::DeviceParameterPair { device, .. } => !alive.contains(&device),
        GestureTarget::DeviceParameterSnapshot { device }
        | GestureTarget::PluginParameter { device, .. }
        | GestureTarget::EqNodeCreation { device } => !alive.contains(&device),
        GestureTarget::ReturnSend { .. }
        | GestureTarget::TrackGain { .. }
        | GestureTarget::TrackGains { .. }
        | GestureTarget::TrackBalance { .. }
        | GestureTarget::MainGain
        | GestureTarget::ReturnGain { .. } => false,
    }) || stale_keys.iter().any(|key| {
        matches!(
            view.device_panel.controls.get(key),
            Some(ParameterControl::Float { entity, .. })
                if entity.read_with(cx, |control, _| {
                    control.is_interacting() || control.is_editing()
                })
        )
    });
    if !stale_keys.is_empty() {
        for key in stale_keys {
            if let Some(mut control) = view.device_panel.controls.remove(&key) {
                cancel_control(view, cx, key, &mut control);
            }
        }
        view.device_panel
            .controls
            .retain(|key, _| alive.contains(&key.device));
        if stale_target_has_work {
            view.device_panel.message = Some("device target is no longer available".into());
        } else if view.device_panel.message.as_deref()
            == Some("device target is no longer available")
        {
            view.device_panel.message = None;
        }
    }
    view.device_panel.views.retain(|id, _| alive.contains(id));
    retire_transfer_previews(&mut view.device_panel, |device| {
        identities
            .get(&device)
            .filter(|identity| identity.class == NativeDeviceClass::Effect)
            .map(|identity| identity.type_id)
    });
    view.device_panel
        .pending_values
        .retain(|key, _| alive.contains(&key.device) && !changed_device_ids.contains(&key.device));
    view.device_panel.operator_pages.retain(|device, _| {
        identities.get(device).is_some_and(|identity| {
            identity.class == NativeDeviceClass::Instrument
                && identity.type_id.as_str().ok() == Some("hex.instrument.operator")
        })
    });
    view.device_panel
        .operator_navigation_focus
        .retain(|(device, _), _| view.device_panel.operator_pages.contains_key(device));
    view.device_panel
        .operator_section_focus
        .retain(|(device, _), _| view.device_panel.operator_pages.contains_key(device));
    let alive_operator: HashSet<_> = identities
        .iter()
        .filter_map(|(device, identity)| {
            (identity.class == NativeDeviceClass::Instrument
                && identity.type_id.as_str().ok() == Some("hex.instrument.operator"))
            .then_some(*device)
        })
        .collect();
    operator_panel::preview::cancel_hidden(view, &alive_operator);
    operator_panel::envelope::retire_removed(view, cx, &alive_operator);
    operator_panel::harmonics::retire(view, cx, |key| alive_operator.contains(&key.0));
    let alive_eq8: HashSet<_> = identities
        .iter()
        .filter_map(|(device, identity)| {
            (identity.class == NativeDeviceClass::Effect
                && matches!(identity.type_id.as_str(), Ok("hex.eq8" | "hex.eq")))
            .then_some(*device)
        })
        .collect();
    retire_removed_eq8_graphs(view, cx, &alive_eq8);
    view.device_panel
        .eq8_panels
        .retain(|device, _| alive_eq8.contains(device));
    view.device_panel
        .eq8_responses
        .retain(|device, _| alive_eq8.contains(device));
    view.device_panel
        .eq8_graph_gestures
        .retain(|device, _| alive_eq8.contains(device));

    // Prepare the full read-only inventory, including unavailable descriptors
    // and unsupported-state errors, without constructing offscreen controls.
    for node in nodes.iter().copied() {
        let (device, class) = match node {
            hex_project::TrackDeviceRef::Instrument(device) => {
                (device, NativeDeviceClass::Instrument)
            }
            hex_project::TrackDeviceRef::Effect(device) => (device, NativeDeviceClass::Effect),
        };
        cache_device_metadata(&mut view.device_panel, device, class);
    }

    parameter_rows::reconcile(view, cx);

    if view.device_panel.effect_catalog.is_none() {
        view.device_panel.effect_catalog = Some(
            hex_engine::native_device_catalog_for_class(NativeDeviceClass::Effect).map_err(
                |error| hex_session::service::text::lowercase_outside_verbatim(&error.to_string()),
            ),
        );
    }
    if view.device_panel.instrument_catalog.is_none() {
        view.device_panel.instrument_catalog = Some(
            hex_engine::native_device_catalog_for_class(NativeDeviceClass::Instrument).map_err(
                |error| hex_session::service::text::lowercase_outside_verbatim(&error.to_string()),
            ),
        );
    }
    let effect_catalog = view
        .device_panel
        .effect_catalog
        .clone()
        .unwrap_or_else(|| Ok(Vec::new()));
    let instrument_catalog = view
        .device_panel
        .instrument_catalog
        .clone()
        .unwrap_or_else(|| Ok(Vec::new()));

    let mut root = div()
        .id("device-panel")
        .debug_selector(|| "device-panel".into())
        .h(px(DEVICE_PANEL_HEIGHT))
        .w_full()
        .flex_shrink_0()
        .flex()
        .flex_col()
        .relative()
        .group("sampler-drop-panel")
        .on_drop(crate::editor::sampler_load::track_drop_listener(
            cx,
            Some(track_id),
            hex_project::InstrumentLoadPlacement::Append,
        ))
        .bg(rgb(theme::color(theme::PaletteRole::Surface)))
        .border_t_1()
        .border_color(rgb(theme::color(theme::PaletteRole::Border)));

    let mut chain = chain_window::rail(view, cx)
        .id("device-chain-scroll")
        .track_scroll(&view.device_panel.chain_window.scroll)
        .relative()
        .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.notify()))
        .debug_selector(|| "device-chain-scroll".into())
        .flex_1()
        .min_h_0()
        .flex()
        .p(px(DEVICE_CHAIN_PADDING))
        .overflow_x_scroll()
        .overflow_y_hidden();
    chain = navigation::rail(view, chain);
    if retained_source_path_label(track).is_some() {
        chain = chain
            .border_t_1()
            .border_color(rgb(theme::color(theme::PaletteRole::Border)));
    }
    let chain_entity = cx.entity();
    let visible_chain_bounds = std::rc::Rc::new(std::cell::RefCell::new(None));
    let chain_visible_bounds = visible_chain_bounds.clone();
    chain = chain.on_drag_move::<drag::NativeDeviceDrag>(move |event, _, cx| {
        let Some(payload) = event
            .dragged_item()
            .downcast_ref::<drag::NativeDeviceDrag>()
        else {
            return;
        };
        let pointer = event.event.position;
        let bounds = event.bounds;
        *chain_visible_bounds.borrow_mut() = Some(bounds);
        let inside = bounds.contains(&pointer);
        if inside {
            payload.return_destination.set(None);
        }
        if !inside && payload.destination.take().is_some() {
            chain_entity.update(cx, |_, cx| cx.notify());
        }
    });

    let device_ids: Vec<_> = track
        .ordered_device_chain()
        .map(|node| match node {
            hex_project::TrackDeviceRef::Instrument(device)
            | hex_project::TrackDeviceRef::Effect(device) => device.id(),
        })
        .collect();
    let geometry: Vec<_> = nodes
        .iter()
        .map(|node| {
            let (device, class) = match node {
                hex_project::TrackDeviceRef::Instrument(device) => {
                    (*device, NativeDeviceClass::Instrument)
                }
                hex_project::TrackDeviceRef::Effect(device) => (*device, NativeDeviceClass::Effect),
            };
            let width = if is_source_effect(device.id()) {
                modulators::SOURCE_EFFECT_CARD_WIDTH
            } else {
                device_card_width(view, class, device)
            };
            (device.id(), width)
        })
        .collect();
    let proposed = view.proposed_plugin_slot_index(
        DevicePanelOwner::Track(track_id),
        geometry.iter().map(|&(id, _)| id),
    );
    view.device_panel.chain_window.prepare(&geometry, proposed);
    if let Some(window) = window.as_deref_mut() {
        view.device_panel
            .chain_window
            .apply_focus_after_prepare(window, cx);
    }
    let window = window.as_deref();
    let spectrum_identities: Vec<_> = nodes
        .iter()
        .filter_map(|node| {
            let hex_project::TrackDeviceRef::Effect(device) = node else {
                return None;
            };
            (!is_source_effect(device.id())
                && !view.device_panel.collapsed.contains(&device.id())
                && eq_panel::is_spectrum_eq(device.state().type_id())
                && chain_window::must_mount(view, device.id(), window, cx))
            .then(|| {
                hex_engine::PostNodeIdentity::new(
                    hex_engine::ProcessorChainOwner::Track(track_id.value()),
                    device.id().value(),
                    device.state().type_id(),
                )
            })
        })
        .collect();
    eq_panel::sync_visible_spectra(view, &spectrum_identities);
    let first_device = device_ids.first().copied();
    let sample_drop = view.browser.active_audio_drag.as_ref().is_some_and(|drag| {
        view.browser_sampler_drop_track(drag, Some(track_id), cx)
            .is_ok()
    });
    let mut local_index = 0;
    let mut source_housing: Option<Div> = None;
    for (physical_index, node) in nodes.into_iter().enumerate() {
        if proposed == Some(physical_index) {
            if let Some(group) = source_housing.take() {
                chain = chain.children([finish_source_housing(group)]);
            }
            chain = chain.children([view.proposed_plugin_slot_card(cx)]);
        }
        let (device, class) = match node {
            hex_project::TrackDeviceRef::Instrument(device) => {
                (device, NativeDeviceClass::Instrument)
            }
            hex_project::TrackDeviceRef::Effect(device) => (device, NativeDeviceClass::Effect),
        };
        let mounted = chain_window::must_mount(view, device.id(), window, cx);
        if is_source_effect(device.id()) {
            let width = modulators::SOURCE_EFFECT_CARD_WIDTH;
            let card = if mounted {
                modulators::source_effect_card(view, cx, device)
            } else {
                chain_window::spacer(device.id(), width)
            };
            let card = card.mr(px(DEVICE_CHAIN_PADDING));
            if let Some(group) = source_housing.take() {
                source_housing = Some(group.child(card));
            } else {
                chain = chain.child(card);
            }
            continue;
        }
        if class != NativeDeviceClass::Instrument
            && let Some(group) = source_housing.take()
        {
            let group = finish_source_housing(group);
            chain = chain.child(group);
        }
        let index = local_index;
        local_index += 1;
        if !mounted {
            let card = chain_window::spacer(device.id(), device_card_width(view, class, device))
                .mr(px(DEVICE_CHAIN_PADDING));
            if class == NativeDeviceClass::Instrument && track.source_id().is_some() {
                source_housing = Some(source_housing_element().child(card));
            } else {
                chain = chain.child(card);
            }
            continue;
        }
        let rendered = match class {
            NativeDeviceClass::Instrument => render_instrument(view, window, cx, track_id, device),
            NativeDeviceClass::Effect => render_effect(view, window, cx, track_id, device),
        };
        let focus = view.device_panel.chain_window.focus(device.id(), cx);
        let card = render_device_card_target(
            view,
            cx,
            DeviceCardDropGeometry {
                track: track_id,
                device: device.id(),
                class,
                width: device_card_width(view, class, device),
                next: device_ids.get(index + 1).copied(),
                first: first_device,
                visible_chain_bounds: visible_chain_bounds.clone(),
                is_sampler: class == NativeDeviceClass::Instrument
                    && device.state().type_id().as_str() == Ok("hex.instrument.sampler"),
                replace_vst3: class == NativeDeviceClass::Effect
                    && device.state().type_id() == hex_lattice_contract::VST3_DEVICE_TYPE_ID
                    && view
                        .device_panel
                        .active_native_drag
                        .as_ref()
                        .is_some_and(|drag| drag.source.is_catalog_vst3_effect())
                    && is_track_vst3_replacement_leaf(track, device.id()),
            },
            rendered,
        )
        .track_focus(&focus);
        let card = keyboard::card(view, cx, device.id(), focus.clone(), card);
        let card = crate::editor::device_comparison::card(
            view,
            cx,
            device.id(),
            class,
            focus.clone(),
            card,
        );
        let mut card =
            crate::editor::device_preset::card(view, cx, device.id(), class, focus, card);
        if sample_drop && class == NativeDeviceClass::Instrument {
            let line = sample_insertion_marker();
            card = card.child(line);
        }
        if class == NativeDeviceClass::Instrument && track.source_id().is_some() {
            source_housing = Some(source_housing_element().child(card));
        } else {
            chain = chain.child(card);
        }
    }
    if let Some(group) = source_housing {
        let group = finish_source_housing(group);
        chain = chain.child(group);
    }
    if proposed == Some(geometry.len()) {
        chain = chain.children([view.proposed_plugin_slot_card(cx)]);
    }
    let mut panel = render_device_end_target(
        view,
        cx,
        drag::NativeDeviceDropTarget {
            track: track_id,
            before: None,
            replace: None,
        },
        first_device,
        visible_chain_bounds,
    );
    if sample_drop && track.instrument().is_none() {
        let line = sample_insertion_marker();
        panel = panel.child(line);
    }
    chain = chain.child(panel);

    let add_open = view.device_panel.add_menu_open;
    let add_button = div()
        .id("device-add-toggle")
        .debug_selector(|| "device-add-toggle".into())
        .size(px(18.0))
        .flex()
        .items_center()
        .justify_center()
        .text_color(rgb(theme::color(theme::PaletteRole::SecondaryText)))
        .hover(|style| style.bg(rgb(theme::color(theme::PaletteRole::Raised))))
        .tooltip(|_, cx| {
            cx.new(|_| crate::editor::transport::MiniTooltip {
                kind: crate::tooltips::Kind::Identification,
                description: "add native device".into(),
            })
            .into()
        })
        .child(theme::text(if add_open { "×" } else { "+" }))
        .on_click(cx.listener(|view, _, window, cx| {
            view.device_panel.add_menu_target = None;
            if view.device_panel.add_menu_open {
                view.device_panel.add_menu_open = false;
                view.focus.focus(window);
            } else {
                device_picker::open(view, window, cx);
            }
            cx.notify();
        }));
    let title = view
        .device_panel
        .message
        .clone()
        .unwrap_or_else(|| format!("{} · devices", track.name()));
    let track_name_view = title_label(title, view.device_panel.message.is_some());
    let mut header = div()
        .id("device-panel-header")
        .debug_selector(|| "device-panel-header".into())
        .h(px(DEVICE_PANEL_HEADER_HEIGHT))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(5.0))
        .px(px(5.0))
        .text_xs()
        .bg(rgb(theme::color(theme::PaletteRole::Surface)));
    if let Some(summary) = view
        .device_panel
        .source_summary
        .get(&state.project, track_id)
    {
        let summary = source_summary_card(summary);
        let card = source_picker::button(view, window, cx, track_id, Some(summary));
        header = header.child(card);
    } else if !track.is_group() && track.source_id().is_none() {
        let card = source_picker::button(view, window, cx, track_id, None);
        header = header.child(card);
    }
    if track.is_group() || !view.track_selection.only_or_empty(track_id) {
        header = header.children(std::iter::once(source_manager::button(view, window, cx)));
    }
    header = header.child(track_name_view);
    if let Some(label) = retained_source_path_label(track) {
        let element = retained_source_path_badge(label);
        header = header.child(element);
    }
    header = header.child(add_button);
    if effect_catalog.is_err() || instrument_catalog.is_err() {
        header = header.child(menu_message("device catalog unavailable"));
    }
    root = root.child(header).child(chain);
    root
}

pub(in crate::editor) fn source_housing_element() -> Div {
    div()
        .debug_selector(|| "source-chain-housing".into())
        .relative()
        .flex()
        .flex_shrink_0()
}

pub(in crate::editor) fn finish_source_housing(group: Div) -> Div {
    // The outline is an overlay so it does not change card offsets used by
    // chain-window virtualization, scrolling, focus or drag geometry.
    group.child(
        div()
            .absolute()
            .inset_0()
            .border_1()
            .border_color(rgb(theme::color(theme::PaletteRole::Border))),
    )
}

pub(in crate::editor) fn source_summary_card(
    summary: &source_summary::SourceSummary,
) -> Stateful<Div> {
    let count = summary.contributors;
    let count_label = if count == 1 {
        "midi track"
    } else {
        "midi tracks"
    };
    let tooltip: gpui::SharedString = source_summary_tooltip(summary).into();
    let background = theme::color(theme::PaletteRole::Surface);
    let label = format!(
        "source · {} · {} · {count} {count_label}",
        hex_session::service::text::verbatim(&summary.name),
        summary.role,
    );
    div()
        .id("device-source-summary")
        .debug_selector(|| "device-source-summary".into())
        .max_w(px(300.0))
        .min_w_0()
        .flex_shrink()
        .truncate()
        .text_xs()
        .text_color(rgb(theme::readable_text(
            background,
            theme::color(theme::PaletteRole::Text),
        )))
        .tooltip(move |_, cx| {
            let description = tooltip.clone();
            cx.new(|_| crate::editor::transport::MiniTooltip {
                kind: crate::tooltips::Kind::Identification,
                description,
            })
            .into()
        })
        .child(theme::text(label))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::editor) enum Vst3PanelPending {
    CurrentDeviceMissing,
    AppliedGraphMissing,
    AppliedGenerationChanged,
    AppliedDeviceMissing,
    ProcessingStateChanged,
    ObservationMissing,
    ProcessorMissing,
}

pub(in crate::editor) fn audible_vst3_processor(
    view: &EditorView,
    device_id: DeviceId,
) -> Result<PreparedDeviceProcessing, Vst3PanelPending> {
    use Vst3PanelPending::*;
    let device = view
        .state
        .as_ref()
        .and_then(|state| state.project.device(device_id))
        .ok_or(CurrentDeviceMissing)?;
    let audible = view.audible_playback.as_ref().ok_or(AppliedGraphMissing)?;
    if audible.project_generation != view.project_generation {
        return Err(AppliedGenerationChanged);
    }
    let applied_device = audible
        .state
        .project
        .device(device_id)
        .ok_or(AppliedDeviceMissing)?;
    if !applied_device.state().processing_eq(device.state()) {
        return Err(ProcessingStateChanged);
    }
    audible
        .observation
        .as_ref()
        .ok_or(ObservationMissing)?
        .processor_status(device_id.value(), device.state().type_id())
        .ok_or(ProcessorMissing)
}

/// Report the same observation that gates the displayed track's VST3 card.
/// The existing view trace retains only changed snapshots and records no
/// preset data or private module paths.
pub(in crate::editor) fn trace_vst3_track_panel(view: &EditorView, snapshot: &mut String) {
    use std::fmt::Write as _;
    if !view.audio_detail.clip_view_open || !view.device_panel.active {
        return;
    }
    let Some(DevicePanelOwner::Track(id)) = displayed_owner(view) else {
        return;
    };
    let Some(track) = view.state.as_ref().and_then(|s| s.project.track(id)) else {
        return;
    };
    let devices = || {
        track.ordered_device_chain().filter_map(|node| {
            let device = match node {
                hex_project::TrackDeviceRef::Instrument(device)
                | hex_project::TrackDeviceRef::Effect(device) => device,
            };
            (device.state().type_id().as_str() == Ok("hex.vst3")).then_some(device.id())
        })
    };
    let Some(device) = view
        .device_panel
        .selected_device
        .and_then(|selected| devices().find(|id| *id == selected))
        .or_else(|| devices().next())
    else {
        return;
    };
    let applied_audio = view
        .audio
        .as_ref()
        .and_then(|audio| audio.control.status())
        .map(|status| status.applied_revision.load(Acquire));
    let _ = write!(
        snapshot,
        " vst3_device={} vst3_applied_audio={applied_audio:?} vst3_observed_audio={:?}",
        device.value(),
        view.audible_playback
            .as_ref()
            .map(|view| view.audio_revision)
    );
    if view.device_panel.degraded_plugins.contains(device.value()) {
        snapshot.push_str(" vst3_panel=degraded");
    } else {
        match audible_vst3_processor(view, device) {
            Ok(PreparedDeviceProcessing::Foreign) => snapshot.push_str(" vst3_panel=active"),
            Ok(PreparedDeviceProcessing::PoweredOff) => {
                snapshot.push_str(" vst3_panel=powered_off")
            }
            Ok(_) => snapshot.push_str(" vst3_panel=unavailable"),
            Err(reason) => {
                let _ = write!(snapshot, " vst3_panel=pending vst3_wait={reason:?}");
            }
        }
    }
}

pub(in crate::editor) fn vst3_status_body(
    view: &mut EditorView,
    device: &hex_project::Device,
    cx: &mut Context<EditorView>,
) -> Stateful<Div> {
    let device_id = device.id();
    if view
        .device_panel
        .degraded_plugins
        .contains(device_id.value())
    {
        return vst3_degraded_body(view, device_id, cx);
    }
    let availability = audible_vst3_processor(view, device_id);
    let parameters = match availability {
        Ok(PreparedDeviceProcessing::Foreign) => Some(vst3_parameters::body(view, device_id, cx)),
        Ok(_) => None,
        Err(_) => vst3_parameters::pending_body(view, device_id, cx),
    };
    if let Some(body) = parameters {
        let status = if availability.is_ok() {
            "active"
        } else {
            "pending"
        };
        let body = div()
            .debug_selector(move || format!("device-vst3-{status}-{}", device_id.value()))
            .child(body);
        return div()
            .id(("device-body", device_id.value()))
            .debug_selector(move || format!("device-body-{}", device_id.value()))
            .child(body);
    }
    let (message, status) = match availability {
        Ok(PreparedDeviceProcessing::PoweredOff) => (
            theme::text("vst3 powered off · saved state preserved"),
            "powered-off",
        ),
        Ok(_) => (
            theme::text("vst3 unavailable · saved state preserved"),
            "unavailable",
        ),
        Err(_) => (theme::text("vst3 · awaiting audio status"), "pending"),
    };
    let content = crate::device_composition::unsupported_body(message)
        .debug_selector(move || format!("device-vst3-{status}-{}", device_id.value()));
    div()
        .child(content)
        .id(ElementId::Name(
            format!("device-body-{}", device_id.value()).into(),
        ))
        .debug_selector(move || format!("device-body-{}", device_id.value()))
}

/// A vst3 endpoint that faulted renders as an aligned pass-through while the
/// other devices keep processing (#3977, #13237). Displays the actual failure
/// reason and an actionable per-device reload control without disturbing
/// healthy plugins.
pub(in crate::editor) fn vst3_degraded_body(
    view: &EditorView,
    device_id: DeviceId,
    cx: &mut Context<EditorView>,
) -> Stateful<Div> {
    let selector = format!("device-vst3-degraded-{}", device_id.value());
    let reason = view
        .device_panel
        .degraded_plugins
        .reason(device_id.value())
        .unwrap_or(crate::editor_backend::DEFAULT_DEGRADED_PLUGIN_REASON)
        .to_owned();
    let reloading = view.device_panel.reloading_plugins.contains(&device_id);
    let selected = view.device_panel.is_selected(device_id);
    let reload_button = actions::segment_button(
        format!("device-vst3-reload-{}", device_id.value()),
        if reloading {
            "reloading plugin..."
        } else {
            "reload plugin"
        },
        "restart only this vst3 plugin from its last saved state",
        if reloading {
            theme::DeviceActionRole::Enabled
        } else {
            theme::DeviceActionRole::Normal
        },
        selected,
    )
    .on_click(cx.listener(move |view, _, _, cx| {
        view.reload_plugin_device(device_id, cx);
    }));
    let tooltip_reason = reason.clone();
    let summary = format!("{reason} · audio passes through");
    let content = crate::device_composition::degraded_body(theme::text(summary), reload_button)
        .id(ElementId::Name(selector.clone().into()))
        .debug_selector(move || selector.clone())
        .tooltip(move |_, cx| {
            let description = theme::text(format!(
                "{tooltip_reason} · bypassed while other devices keep playing"
            ));
            cx.new(|_| crate::editor::transport::MiniTooltip {
                kind: crate::tooltips::Kind::Help,
                description,
            })
            .into()
        });
    div()
        .child(content)
        .id(ElementId::Name(
            format!("device-body-{}", device_id.value()).into(),
        ))
        .debug_selector(move || format!("device-body-{}", device_id.value()))
}

pub(in crate::editor) fn render_effect(
    view: &mut EditorView,
    window: Option<&Window>,
    cx: &mut Context<EditorView>,
    track: TrackId,
    device: &hex_project::Device,
) -> Stateful<Div> {
    let device_id = device.id();
    let selected = view.device_panel.is_selected(device_id);
    let device_view = view
        .device_panel
        .views
        .remove(&device_id)
        .filter(|(class, cached, _)| {
            *class == NativeDeviceClass::Effect && cached == device.state()
        })
        .map(|(_, _, result)| result)
        .unwrap_or_else(|| {
            hex_engine::native_device_view_for_class(NativeDeviceClass::Effect, device.state())
                .map_err(|error| {
                    hex_session::service::text::lowercase_outside_verbatim(&error.to_string())
                })
        });
    automation_playback::sample(
        view,
        device,
        NativeDeviceClass::Effect,
        device_view.as_ref().ok(),
    );
    let leading = bypass_action(view, device, NativeDeviceClass::Effect, cx);
    let header_actions = device_actions(view, device, selected, device_view.is_ok(), cx);
    let output = device_meter::render(view, device, cx);
    let rendered = match &device_view {
        _ if view.device_panel.collapsed.contains(&device_id) => fold::frame(
            view,
            cx,
            device,
            NativeDeviceClass::Effect,
            device_view.as_ref().ok(),
            leading,
            header_actions,
        ),
        _ if device.state().type_id().as_str() == Ok("hex.vst3") => {
            let body = vst3_status_body(view, device, cx);
            hex_widgets::device_frame_with_panels(
                device_drag_title(
                    view,
                    cx,
                    device,
                    NativeDeviceClass::Effect,
                    view.vst3_device_title(device),
                ),
                theme::device_frame_style(selected),
                leading,
                header_actions,
                body,
                modulators::vst3_macro_panel(view, cx, device),
                modulators::vst3_panel(view, cx, device),
            )
        }
        Ok(device_view) => {
            let body = if device_view.type_id.as_str() == Ok("hex.utility") {
                utility_panel::render_body(view, cx, device_id, device_view)
            } else if device_view.type_id.as_str() == Ok("hex.limiter") {
                limiter_panel::render_body(view, cx, device, device_view)
            } else if device_view.type_id.as_str() == Ok("hex.compressor") {
                compressor_panel::render_body(view, cx, device, device_view)
            } else if device_view.type_id.as_str() == Ok("hex.saturator") {
                saturator_panel::render_body(view, cx, device_id, device.state(), device_view)
            } else if device_view.type_id.as_str() == Ok("hex.reverb") {
                reverb_panel::render_body(view, cx, device_id, device_view)
            } else if device_view.type_id.as_str() == Ok("hex.delay") {
                delay_panel::render_body(view, cx, device_id, device.state(), device_view)
            } else if device_view.type_id.as_str() == Ok("hex.overdrive") {
                overdrive_panel::render_body(view, cx, device_id, device.state(), device_view)
            } else if device_view.type_id.as_str() == Ok("hex.rattle") {
                rattle_panel::render_body(view, cx, device, device_view)
            } else if device_view.type_id.as_str() == Ok("hex.filter") {
                filter_panel::render_body(view, cx, device, device_view)
            } else if device_view.type_id.as_str() == Ok("hex.gate") {
                gate_panel::render_body(view, cx, device_id, device_view)
            } else if device_view.type_id.as_str() == Ok("hex.auto-pan") {
                auto_pan_panel::render_body(view, cx, device_id, device_view)
            } else if device_view.type_id.as_str() == Ok("hex.eq") {
                let sample_rate = view
                    .state
                    .as_ref()
                    .map_or(48000, |state| state.project.sample_rate());
                eq_panel::render_body(
                    view,
                    cx,
                    device_id,
                    hex_engine::ProcessorChainOwner::Track(track.value()),
                    device.state(),
                    device_view,
                    sample_rate,
                )
            } else if device_view.type_id.as_str() == Ok("hex.eq8") {
                let sample_rate = view
                    .state
                    .as_ref()
                    .map_or(48_000, |state| state.project.sample_rate());
                eq8_panel::render_body(
                    view,
                    cx,
                    device_id,
                    hex_engine::ProcessorChainOwner::Track(track.value()),
                    device.state(),
                    device_view,
                    sample_rate,
                )
            } else {
                parameter_rows::body(
                    view,
                    window,
                    cx,
                    device_id,
                    DeviceControlIdentity {
                        class: NativeDeviceClass::Effect,
                        type_id: device_view.type_id,
                    },
                    device_view,
                )
            };
            hex_widgets::device_frame_with_panels(
                device_drag_title(
                    view,
                    cx,
                    device,
                    NativeDeviceClass::Effect,
                    theme::text(device_view.name.clone()),
                ),
                theme::device_frame_style(selected),
                leading,
                header_actions,
                body,
                modulators::macro_panel(view, cx, device, device_view, true),
                modulators::panel(view, cx, device, device_view, true),
            )
        }
        Err(error) => {
            let body = crate::device_composition::unsupported_body(theme::text(format!(
                "unsupported device state · {error}"
            )))
            .id(ElementId::Name(
                format!("device-body-{}", device_id.value()).into(),
            ))
            .debug_selector(move || format!("device-body-{}", device_id.value()));
            let title = theme::text(
                device
                    .state()
                    .type_id()
                    .as_str()
                    .unwrap_or("unknown device"),
            );
            device_frame(
                device_drag_title(view, cx, device, NativeDeviceClass::Effect, title),
                theme::device_frame_style(selected),
                leading,
                header_actions,
                body,
            )
        }
    };
    let rendered = rendered
        .h_full()
        .debug_selector(move || format!("device-shell-{}", device_id.value()));
    view.device_panel.views.insert(
        device_id,
        (
            NativeDeviceClass::Effect,
            device.state().clone(),
            device_view,
        ),
    );
    let selector = format!("effect-device-{}", device_id.value());
    let card_width = device_card_width(view, NativeDeviceClass::Effect, device);
    let rendered = div()
        .min_w(px(card_width - device_meter::WIDTH))
        .flex_shrink_0()
        .child(rendered);
    div()
        .id(ElementId::Name(selector.clone().into()))
        .debug_selector(move || selector.clone())
        .min_w(px(card_width))
        .h(px(DEVICE_FRAME_HEIGHT))
        .flex()
        .flex_shrink_0()
        .capture_any_mouse_down(device_selection_listener(
            device_id,
            view.project_generation,
            view.device_panel.owner,
            cx,
        ))
        .child(rendered)
        .child(output)
}

pub(in crate::editor) fn render_instrument(
    view: &mut EditorView,
    window: Option<&Window>,
    cx: &mut Context<EditorView>,
    track: TrackId,
    device: &hex_project::Device,
) -> Stateful<Div> {
    let device_id = device.id();
    let selected = view.device_panel.is_selected(device_id);
    let cached = view
        .device_panel
        .views
        .remove(&device_id)
        .filter(|(class, state, _)| {
            *class == NativeDeviceClass::Instrument && state == device.state()
        })
        .map(|(_, _, result)| result);
    let device_view = cached.unwrap_or_else(|| {
        hex_engine::native_device_view_for_class(NativeDeviceClass::Instrument, device.state())
            .map_err(|error| {
                hex_session::service::text::lowercase_outside_verbatim(&error.to_string())
            })
    });
    automation_playback::sample(
        view,
        device,
        NativeDeviceClass::Instrument,
        device_view.as_ref().ok(),
    );
    let is_operator = device.state().type_id().as_str() == Ok("hex.instrument.operator");
    let is_sampler = device.state().type_id().as_str() == Ok("hex.instrument.sampler");
    let leading = bypass_action(view, device, NativeDeviceClass::Instrument, cx);
    let header_actions = instrument_actions(view, track, device, selected, device_view.is_ok(), cx);
    let rendered = if view.device_panel.collapsed.contains(&device_id) {
        fold::frame(
            view,
            cx,
            device,
            NativeDeviceClass::Instrument,
            device_view.as_ref().ok(),
            leading,
            header_actions,
        )
    } else if device.state().type_id().as_str() == Ok("hex.vst3") {
        device_frame(
            device_drag_title(
                view,
                cx,
                device,
                NativeDeviceClass::Instrument,
                view.vst3_device_title(device),
            ),
            theme::device_frame_style(selected),
            leading,
            header_actions,
            vst3_status_body(view, device, cx),
        )
    } else if hex_engine::is_empty_sampler_state_on_control(device.state()) {
        device_frame(
            device_drag_title(
                view,
                cx,
                device,
                NativeDeviceClass::Instrument,
                theme::text("sampler"),
            ),
            theme::device_frame_style(selected),
            leading,
            header_actions,
            crate::device_composition::body()
                .p(px(8.0))
                .child(theme::text(
                    "empty sampler · choose a wav in the sample browser and load into sampler",
                )),
        )
    } else if is_sampler {
        let name = view
            .state
            .as_ref()
            .and_then(|s| {
                let reference = device.state().asset_references().first()?;
                s.project
                    .assets()
                    .find(|a| a.id().value() == reference.asset.value())?
                    .path()
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "missing sample identity".into());
        let readiness = if view.sampler_parameter_pending() {
            "preparing sampler edit · waiting for audio"
        } else if view.first_sampler_current() {
            "audio ready"
        } else {
            "audio not ready · offline export supported"
        };
        let gain = view.sampler_parameter_control(
            hex_session::service::SamplerParameterOwner::Track(track),
            device,
            hex_engine::SamplerParameter::Gain,
            cx,
        );
        let modulation_view = device_view.as_ref().ok().filter(|_| {
            view.document_audio_mode == hex_session::DocumentAudioMode::Resident
                && device.state().asset_references().len() == 1
                && view
                    .state
                    .as_ref()
                    .and_then(|state| state.project.track(track))
                    .is_some_and(hex_project::Track::receives_source_audio)
        });
        let gain_overlay = modulation_view
            .and_then(|metadata| {
                metadata
                    .parameters
                    .iter()
                    .find(|row| row.descriptor.id == hex_engine::SAMPLER_GAIN_PARAMETER)
            })
            .and_then(|target| modulators::target_overlay(view, cx, device_id, target));
        let gain = div()
            .relative()
            .children([gain])
            .children(gain_overlay)
            .into_any_element();
        let pitch = view.sampler_parameter_control(
            hex_session::service::SamplerParameterOwner::Track(track),
            device,
            hex_engine::SamplerParameter::Transpose,
            cx,
        );
        let start = view.sampler_parameter_control(
            hex_session::service::SamplerParameterOwner::Track(track),
            device,
            hex_engine::SamplerParameter::StartFrame,
            cx,
        );
        let end = view.sampler_parameter_control(
            hex_session::service::SamplerParameterOwner::Track(track),
            device,
            hex_engine::SamplerParameter::EndFrame,
            cx,
        );
        let mut fields = vec![gain, pitch, start, end];
        let loop_error = match hex_engine::sampler_parameter_value_on_control(
            hex_engine::SamplerParameter::LoopEnabled,
            device.state(),
        ) {
            Ok(_) => {
                for parameter in [
                    hex_engine::SamplerParameter::LoopEnabled,
                    hex_engine::SamplerParameter::LoopStartFrame,
                    hex_engine::SamplerParameter::LoopEndFrame,
                    hex_engine::SamplerParameter::LoopCrossfadeMs,
                ] {
                    fields.push(view.sampler_parameter_control(
                        hex_session::service::SamplerParameterOwner::Track(track),
                        device,
                        parameter,
                        cx,
                    ));
                }
                None
            }
            Err(error) => Some(error.to_string()),
        };
        for parameter in [
            hex_engine::SamplerParameter::AttackMs,
            hex_engine::SamplerParameter::DecayMs,
            hex_engine::SamplerParameter::SustainPercent,
            hex_engine::SamplerParameter::ReleaseMs,
        ] {
            fields.push(view.sampler_parameter_control(
                hex_session::service::SamplerParameterOwner::Track(track),
                device,
                parameter,
                cx,
            ));
        }
        let controls = div()
            .grid()
            // One dense row: twelve classic controls at the sampler's inline
            // value width (see `SAMPLER_INLINE_VALUE_WIDTH`) fit the 1024 px
            // device body, keeping the waveform overview's full height.
            .grid_cols(if loop_error.is_none() { 12 } else { 8 })
            .gap(px(2.0))
            .w_full()
            .flex_shrink_0()
            .debug_selector(|| "sampler-controls".into())
            .children(fields);
        let details = div()
            .w_full()
            .flex_shrink_0()
            .text_size(px(9.0))
            .line_height(px(12.0))
            .overflow_hidden()
            .child(theme::text(format!(
                "{readiness} · * release inactive in trigger mode{}",
                loop_error.map_or_else(String::new, |error| format!(" · {error}"))
            )));
        let overview = sampler_overview::render(
            view,
            hex_session::service::SamplerParameterOwner::Track(track),
            device,
            &name,
            cx,
        );
        let body = crate::device_composition::body()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .p(px(4.0))
            .text_xs()
            .child(overview)
            .child(controls)
            .child(details);
        let macros = modulation_view
            .and_then(|metadata| modulators::macro_panel(view, cx, device, metadata, false));
        let modulation = modulation_view
            .and_then(|metadata| modulators::panel(view, cx, device, metadata, false));
        hex_widgets::device_frame_with_panels(
            device_drag_title(
                view,
                cx,
                device,
                NativeDeviceClass::Instrument,
                theme::text("sampler"),
            ),
            theme::device_frame_style(selected),
            leading,
            header_actions,
            body,
            macros,
            modulation,
        )
    } else {
        match &device_view {
            Ok(device_view) => {
                if is_operator && device_view.type_id.as_str() == Ok("hex.instrument.operator") {
                    let body = operator_panel::render_body(view, cx, device_id, device_view);
                    let macros = modulators::macro_panel(view, cx, device, device_view, false);
                    let modulation = modulators::panel(view, cx, device, device_view, false);
                    hex_widgets::device_frame_with_panels(
                        device_drag_title(
                            view,
                            cx,
                            device,
                            NativeDeviceClass::Instrument,
                            theme::text(device_view.name.clone()),
                        ),
                        theme::device_frame_style(selected),
                        leading,
                        header_actions,
                        body,
                        macros,
                        modulation,
                    )
                } else {
                    let body = parameter_rows::body(
                        view,
                        window,
                        cx,
                        device_id,
                        DeviceControlIdentity {
                            class: NativeDeviceClass::Instrument,
                            type_id: device_view.type_id,
                        },
                        device_view,
                    );
                    let fm4_source = device_view.type_id.as_str() == Ok("hex.instrument.fm4")
                        && view
                            .state
                            .as_ref()
                            .and_then(|state| state.project.track(track))
                            .is_some_and(hex_project::Track::receives_source_audio);
                    let macros = fm4_source
                        .then(|| modulators::macro_panel(view, cx, device, device_view, false))
                        .flatten();
                    let modulation = fm4_source
                        .then(|| modulators::panel(view, cx, device, device_view, false))
                        .flatten();
                    hex_widgets::device_frame_with_panels(
                        device_drag_title(
                            view,
                            cx,
                            device,
                            NativeDeviceClass::Instrument,
                            theme::text(device_view.name.clone()),
                        ),
                        theme::device_frame_style(selected),
                        leading,
                        header_actions,
                        body,
                        macros,
                        modulation,
                    )
                }
            }
            Err(error) => {
                let body = crate::device_composition::unsupported_body(theme::text(format!(
                    "unsupported instrument state · {error}"
                )))
                .id(ElementId::Name(
                    format!("device-body-{}", device_id.value()).into(),
                ))
                .debug_selector(move || format!("device-body-{}", device_id.value()));
                device_frame(
                    device_drag_title(
                        view,
                        cx,
                        device,
                        NativeDeviceClass::Instrument,
                        theme::text(
                            device
                                .state()
                                .type_id()
                                .as_str()
                                .unwrap_or("unknown instrument"),
                        ),
                    ),
                    theme::device_frame_style(selected),
                    leading,
                    header_actions,
                    body,
                )
            }
        }
    };
    let rendered = rendered
        .h_full()
        .debug_selector(move || format!("device-shell-{}", device_id.value()));
    view.device_panel.views.insert(
        device_id,
        (
            NativeDeviceClass::Instrument,
            device.state().clone(),
            device_view,
        ),
    );
    let selector = format!("instrument-device-{}", device_id.value());
    let card_width = device_card_width(view, NativeDeviceClass::Instrument, device);
    let rendered = div()
        .min_w(px(card_width - device_meter::WIDTH))
        .flex_shrink_0()
        .child(rendered);
    div()
        .id(ElementId::Name(selector.clone().into()))
        .debug_selector(move || selector.clone())
        .min_w(px(card_width))
        .h(px(DEVICE_FRAME_HEIGHT))
        .flex()
        .flex_shrink_0()
        .capture_any_mouse_down(device_selection_listener(
            device_id,
            view.project_generation,
            view.device_panel.owner,
            cx,
        ))
        .child(rendered)
        .child(device_meter::render(view, device, cx))
}

pub(in crate::editor) fn render_automation_trim(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
) -> Option<Div> {
    use hex_lattice_contract::DeviceAutomationTarget;
    let target = view.selected_automation_trim()?;
    let descriptor = match target.kind {
        DeviceAutomationTarget::HostInputGain => hex_engine::DEVICE_INPUT_GAIN,
        DeviceAutomationTarget::HostOutputGain => hex_engine::DEVICE_OUTPUT_GAIN,
        _ => return None,
    };
    let state = view.state.as_ref()?;
    let device = target.current(&state.project, view.project_generation)?;
    let identity = DeviceControlIdentity {
        class: target.class,
        type_id: device.state().type_id(),
    };
    let value =
        hex_engine::native_device_parameter_for_class(target.class, device.state(), descriptor.id)
            .ok()?;
    let control = bindings::render_parameter(
        view,
        cx,
        target.device,
        identity,
        &NativeDeviceParameterView {
            descriptor,
            semantics: None,
            value,
            enabled: true,
        },
        bindings::BindingPresentation {
            kind: ControlPresentation::InlineValue,
            text: ControlText::default(),
            policy: binding_policy,
            compact_toggle_width: 0.0,
        },
    );
    Some(
        div()
            .flex()
            .items_center()
            .gap(px(3.0))
            .flex_shrink_0()
            .child(theme::text(descriptor.label))
            .child(control),
    )
}

pub(in crate::editor) fn menu_message(message: &str) -> Div {
    div()
        .h(px(18.0))
        .flex()
        .items_center()
        .px(px(4.0))
        .text_xs()
        .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
        .child(theme::text(message))
}

pub(in crate::editor) fn empty_panel(message: &str) -> Stateful<Div> {
    div()
        .id("device-panel-empty")
        .debug_selector(|| "device-panel-empty".into())
        .h(px(DEVICE_PANEL_HEIGHT))
        .w_full()
        .flex_shrink_0()
        .flex()
        .items_center()
        .px(px(8.0))
        .bg(rgb(theme::color(theme::PaletteRole::Surface)))
        .text_xs()
        .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
        .child(theme::text(message))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vst3_panel_pending_variants_are_distinct() {
        assert_ne!(
            Vst3PanelPending::CurrentDeviceMissing,
            Vst3PanelPending::ProcessorMissing
        );
        assert_ne!(
            Vst3PanelPending::AppliedGraphMissing,
            Vst3PanelPending::ProcessingStateChanged
        );
    }
}
