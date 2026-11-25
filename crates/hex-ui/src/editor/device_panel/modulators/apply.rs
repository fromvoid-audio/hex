//! Modulator state mutation and canonical edit submission.
use super::*;

pub(in crate::editor) fn submit(
    view: &mut EditorView,
    generation: u64,
    revision: u64,
    device: DeviceId,
    edit: Edit,
) {
    let Some(state) = view
        .state
        .as_ref()
        .filter(|state| view.project_generation == generation && state.revision == revision)
    else {
        return;
    };
    match apply(&state.project, device, edit) {
        Ok(modulation) => {
            view.enqueue(Request::ReplaceDeviceModulation {
                project_generation: generation,
                revision,
                device,
                modulation,
            });
        }
        Err(error) => view.error = Some(error.0.into()),
    }
}

pub(in crate::editor) fn apply(
    project: &hex_project::Project,
    id: DeviceId,
    edit: Edit,
) -> Result<DeviceModulation, Invalid> {
    let device = project
        .device(id)
        .ok_or(Invalid("device no longer exists"))?;
    let bank = device.modulation();
    let mut sources = bank.sources().to_vec();
    let mut routes = bank.routes().to_vec();
    let unsupported = |_| Invalid("modulator settings are unsupported");
    let maximum_hz = f64::from(project.sample_rate()) * 0.5;
    match edit {
        Edit::Add(kind) => {
            if kind.required_outputs() > hex_engine::device_available_modulation_outputs(device) {
                return Err(Invalid("device modulator output capacity reached"));
            }
            if matches!(kind, SourceKind::NotePressure)
                && !hex_engine::project_device_can_add_note_pressure(project, id)
            {
                return Err(Invalid(
                    "one note pressure source is supported on an fm4 receiver",
                ));
            }
            if matches!(
                kind,
                SourceKind::NoteCounter
                    | SourceKind::Ramp
                    | SourceKind::AhdRelease
                    | SourceKind::Adsr
            ) && !hex_engine::project_device_has_modulation_note_events(project, id)
            {
                return Err(Invalid("this modulator requires instrument note events"));
            }
            sources.push(Modulator {
                color: Default::default(),
                id: ModulationSourceId::new(project.next_identity()).map_err(unsupported)?,
                enabled: true,
                state: kind
                    .default_state(project.next_identity())
                    .map_err(unsupported)?,
            });
        }
        Edit::ToggleSource(id) => {
            let source = find_source(&mut sources, id)?;
            source.enabled = !source.enabled;
        }
        Edit::ToggleButton(id) => {
            let source = find_source(&mut sources, id)?;
            let on = manual_button::decode(&source.state).map_err(unsupported)?;
            source.state = manual_button::encode(!on).map_err(unsupported)?;
        }
        Edit::ToggleButtons(output) => {
            let source = find_source(&mut sources, output.source)?;
            let mut settings = buttons::Settings::decode(&source.state).map_err(unsupported)?;
            let index = buttons::index_for_output(output.output)
                .ok_or(Invalid("button output is unsupported"))?;
            settings.on[index] = !settings.on[index];
            source.state = settings.encode().map_err(unsupported)?;
        }
        Edit::NoteCounter(id, edit) => {
            let source = find_source(&mut sources, id)?;
            let mut settings =
                note_counter::Settings::decode(&source.state).map_err(unsupported)?;
            match edit {
                CounterEdit::Steps(delta) => {
                    settings.steps = settings.steps.saturating_add_signed(delta).clamp(2, 128);
                }
                CounterEdit::Increment(delta) => {
                    settings.increment = settings
                        .increment
                        .saturating_add_signed(delta)
                        .clamp(1, 128);
                }
                CounterEdit::TogglePolarity => settings.bipolar = !settings.bipolar,
            }
            source.state = settings.encode().map_err(unsupported)?;
        }
        Edit::Ramp(id, edit) => {
            let source = find_source(&mut sources, id)?;
            let mut settings = ramp::Settings::decode(&source.state).map_err(unsupported)?;
            match edit {
                RampEdit::Duration { longer } => {
                    settings.duration_micros =
                        duration_step(settings.duration_micros, longer).max(ramp::MIN_MICROS);
                }
                RampEdit::Curve(curve) => settings.curve = curve,
                RampEdit::ToggleDirection => settings.falling = !settings.falling,
                RampEdit::ToggleLoop => settings.looping = !settings.looping,
            }
            source.state = settings.encode().map_err(unsupported)?;
        }
        Edit::AhdRelease(id, edit) => {
            let source = find_source(&mut sources, id)?;
            let mut settings = ahd_release::Settings::decode(&source.state).map_err(unsupported)?;
            match edit {
                AhdEdit::Duration { segment, longer } => {
                    let micros = settings
                        .durations_micros
                        .get_mut(segment)
                        .ok_or(Invalid("envelope segment is unsupported"))?;
                    *micros = duration_step(*micros, longer);
                    if settings.durations_micros == [0; 3] {
                        settings.durations_micros[segment] = ramp::MIN_MICROS;
                    }
                }
                AhdEdit::Curve { index, curve } => {
                    let control = settings
                        .curves
                        .get_mut(index)
                        .ok_or(Invalid("envelope curve is unsupported"))?;
                    *control = curve;
                }
                AhdEdit::ToggleSingleTrigger => settings.single_trigger = !settings.single_trigger,
            }
            source.state = settings.encode().map_err(unsupported)?;
        }
        Edit::Adsr(id, edit) => {
            let source = find_source(&mut sources, id)?;
            let mut settings = adsr::Settings::decode(&source.state).map_err(unsupported)?;
            match edit {
                AdsrEdit::Duration { segment, longer } => {
                    let duration = settings
                        .durations_micros
                        .get_mut(segment)
                        .ok_or(Invalid("envelope segment is unsupported"))?;
                    *duration = duration_step(*duration, longer);
                }
                AdsrEdit::Sustain(delta) => {
                    settings.sustain = (settings.sustain + delta).clamp(0.0, 1.0);
                }
                AdsrEdit::ToggleSingleTrigger => settings.single_trigger = !settings.single_trigger,
            }
            source.state = settings.encode().map_err(unsupported)?;
        }
        Edit::Steps(id, edit) => {
            let source = find_source(&mut sources, id)?;
            let mut settings = steps::Settings::decode(&source.state).map_err(unsupported)?;
            match edit {
                StepsEdit::Time { longer } => {
                    settings.timing = match settings.timing {
                        steps::Timing::FreeMicros(micros) => steps::Timing::FreeMicros(
                            duration_step(micros, longer).max(ramp::MIN_MICROS),
                        ),
                        steps::Timing::Arrangement { steps_per_beat } => {
                            let steps_per_beat = if longer {
                                (steps_per_beat / 2).max(1)
                            } else {
                                (steps_per_beat * 2).min(steps::MAX_STEPS_PER_BEAT)
                            };
                            steps::Timing::Arrangement { steps_per_beat }
                        }
                    };
                }
                StepsEdit::ToggleTiming => {
                    settings.timing = match settings.timing {
                        steps::Timing::FreeMicros(_) => steps::Timing::DEFAULT_ARRANGEMENT,
                        steps::Timing::Arrangement { .. } => steps::Timing::DEFAULT_FREE,
                    };
                }
                StepsEdit::Length { add } => {
                    let count = settings.pattern.len();
                    let count = if add { count + 1 } else { count - 1 };
                    settings
                        .pattern
                        .resize(count.clamp(1, steps::MAX_MODULATION_STEPS), 0);
                }
                StepsEdit::Value { index, delta } => {
                    let value = settings
                        .pattern
                        .get_mut(index)
                        .ok_or(Invalid("step is unsupported"))?;
                    *value = value.saturating_add(delta).max(-i16::MAX);
                }
                StepsEdit::CycleDirection => {
                    settings.direction = match settings.direction {
                        steps::Direction::Forward => steps::Direction::Backward,
                        steps::Direction::Backward => steps::Direction::PingPong,
                        steps::Direction::PingPong => steps::Direction::Forward,
                    };
                }
            }
            source.state = settings.encode().map_err(unsupported)?;
        }
        Edit::DuplicateSource(id) => {
            if is_pressure_source(&sources, id) {
                return Err(Invalid(
                    "one note pressure source is supported on an fm4 receiver",
                ));
            }
            return bank.duplicate_source(id, project.next_identity());
        }
        Edit::RemoveSource(id) => {
            sources.retain(|source| source.id != id);
            routes.retain(|route| {
                route.source != id
                    && !matches!(route.destination, ModulationDestination::Modulator { source, .. } if source == id)
            });
        }
        Edit::Rate(id, factor) => {
            let source = find_source(&mut sources, id)?;
            let mut settings = lfo::Settings::decode(&source.state).map_err(unsupported)?;
            settings.rate = match settings.rate {
                lfo::Rate::Hertz(rate) => lfo::Rate::Hertz((rate * factor).clamp(0.01, maximum_hz)),
                lfo::Rate::BeatsPerCycle(rate) => {
                    lfo::Rate::BeatsPerCycle((rate / factor).clamp(0.0625, 64.0))
                }
            };
            source.state = settings.encode().map_err(unsupported)?;
        }
        Edit::Sync(id) => {
            let source = find_source(&mut sources, id)?;
            let mut settings = lfo::Settings::decode(&source.state).map_err(unsupported)?;
            let beats_per_second = f64::from(
                project
                    .tempo_map()
                    .tempo_at(hex_project::BeatTime::ZERO)
                    .micro_bpm(),
            ) / 60e6;
            settings.rate = match settings.rate {
                lfo::Rate::Hertz(rate) => {
                    lfo::Rate::BeatsPerCycle((beats_per_second / rate).clamp(0.0625, 64.0))
                }
                lfo::Rate::BeatsPerCycle(beats) => {
                    lfo::Rate::Hertz((beats_per_second / beats).clamp(0.01, maximum_hz))
                }
            };
            source.state = settings.encode().map_err(unsupported)?;
        }
        Edit::LfoShape(id, waveform) => {
            let source = find_source(&mut sources, id)?;
            let mut settings = lfo::Settings::decode(&source.state).map_err(unsupported)?;
            settings.waveform = waveform;
            source.state = settings.encode().map_err(unsupported)?;
        }
        Edit::MacroValue(id, change) => {
            let source = find_source(&mut sources, id)?;
            let mut settings = manual::Settings::decode(&source.state).map_err(unsupported)?;
            settings.value = (settings.value + change).clamp(0.0, 1.0);
            source.state = settings.encode().map_err(unsupported)?;
        }
        Edit::MacroFourValue(id, parameter, change) => {
            let source = find_source(&mut sources, id)?;
            let mut settings = macro_four::Settings::decode(&source.state).map_err(unsupported)?;
            let index = macro_four::VALUE_PARAMETERS
                .iter()
                .position(|&candidate| candidate == parameter)
                .ok_or(Invalid("macro-4 control is unsupported"))?;
            let value = settings
                .values
                .get_mut(index)
                .ok_or(Invalid("macro-4 control is unsupported"))?;
            *value = (*value + change).clamp(0.0, 1.0);
            source.state = settings.encode().map_err(unsupported)?;
        }
        Edit::CompositionValue(id, parameter, change) => {
            let source = find_source(&mut sources, id)?;
            let unsupported_input = Invalid("modulator input is unsupported");
            source.state = match source.state.type_id() {
                math::TYPE_ID => {
                    let mut settings =
                        math::Settings::decode(&source.state).map_err(unsupported)?;
                    let value = match parameter {
                        math::INPUT_A => &mut settings.inputs[0],
                        math::INPUT_B => &mut settings.inputs[1],
                        _ => return Err(unsupported_input),
                    };
                    *value = (*value + change).clamp(-1.0, 1.0);
                    settings.encode()
                }
                mix::TYPE_ID => {
                    let mut settings = mix::Settings::decode(&source.state).map_err(unsupported)?;
                    let (value, minimum) = match parameter {
                        mix::INPUT_A => (&mut settings.inputs[0], -1.0),
                        mix::INPUT_B => (&mut settings.inputs[1], -1.0),
                        mix::BLEND => (&mut settings.blend, 0.0),
                        _ => return Err(unsupported_input),
                    };
                    *value = (*value + change).clamp(minimum, 1.0);
                    settings.encode()
                }
                _ => return Err(unsupported_input),
            }
            .map_err(unsupported)?;
        }
        Edit::MathOperation(id, operation) => {
            let source = find_source(&mut sources, id)?;
            let mut settings = math::Settings::decode(&source.state).map_err(unsupported)?;
            settings.operation = operation;
            source.state = settings.encode().map_err(unsupported)?;
        }
        Edit::FollowerTime {
            source,
            attack,
            factor,
        } => {
            let source = find_source(&mut sources, source)?;
            let mut settings = follower::Settings::decode(&source.state).map_err(unsupported)?;
            let time = if attack {
                &mut settings.attack_seconds
            } else {
                &mut settings.release_seconds
            };
            *time = (*time * factor).clamp(0.0001, 10.0);
            source.state = settings.encode().map_err(unsupported)?;
        }
        Edit::RandomRate(id, factor) => {
            let source = find_source(&mut sources, id)?;
            let mut settings = random::Settings::decode(&source.state).map_err(unsupported)?;
            settings.rate_hz = (settings.rate_hz * factor).clamp(0.01, maximum_hz);
            source.state = settings.encode().map_err(unsupported)?;
        }
        Edit::Reseed(id) => {
            let source = find_source(&mut sources, id)?;
            let mut settings = random::Settings::decode(&source.state).map_err(unsupported)?;
            settings.seed = settings.seed.wrapping_add(0xd1b5_4a32_d192_ed03);
            source.state = settings.encode().map_err(unsupported)?;
        }
        Edit::Map(source, target, depth) => {
            if !sources.iter().any(|slot| source.valid_for(slot)) {
                return Err(Invalid("modulator source output is unsupported"));
            }
            let pressure = is_pressure_source(&sources, source.source);
            if pressure
                && (!matches!(target, MappingTarget::Device(id) if hex_engine::fm4_note_pressure_parameter_supported(id))
                    || routes.iter().any(|route| source.owns(route)))
            {
                return Err(Invalid("note pressure supports one carrier assignment"));
            }
            let destination = match target {
                MappingTarget::Device(parameter) => {
                    if !hex_engine::project_device_modulation_target_supported(
                        project, id, parameter,
                    ) {
                        return Err(Invalid(
                            "this parameter does not support sample-rate modulation",
                        ));
                    }
                    ModulationDestination::Device {
                        device_type: device.state().type_id(),
                        target: DeviceAutomationTarget::Parameter(parameter),
                        vst3_class: None,
                    }
                }
                MappingTarget::Vst3(target) => {
                    vst3_destination(project, id, target.parameter, target.class)?
                }
                MappingTarget::HostGain(stage) => {
                    let available = hex_engine::project_device_host_gain_modulation_supported(
                        project, id,
                    ) || (matches!(stage, HostGainTarget::Output)
                        && hex_engine::project_device_instrument_output_modulation_supported(
                            project, id,
                        ));
                    if !available {
                        return Err(Invalid("this device does not support this host gain stage"));
                    }
                    ModulationDestination::Device {
                        device_type: device.state().type_id(),
                        target: stage.mapping().0,
                        vst3_class: None,
                    }
                }
                MappingTarget::Macro(target) => macro_destination(target),
                MappingTarget::MacroFour(target, parameter) => {
                    let target = find_source(&mut sources, target)?;
                    macro_four::Settings::decode(&target.state).map_err(unsupported)?;
                    macro_four::output_for_parameter(parameter)
                        .ok_or(Invalid("macro-4 control is unsupported"))?;
                    macro_four_destination(target.id, parameter)
                }
                MappingTarget::CompositionInput(target, parameter) => {
                    let target = find_source(&mut sources, target)?;
                    let module_type = target.state.type_id();
                    match module_type {
                        math::TYPE_ID => {
                            math::Settings::decode(&target.state).map_err(unsupported)?;
                        }
                        mix::TYPE_ID => {
                            mix::Settings::decode(&target.state).map_err(unsupported)?;
                        }
                        _ => return Err(Invalid("modulator input is unsupported")),
                    }
                    composition_input_name(module_type, parameter)
                        .ok_or(Invalid("modulator input is unsupported"))?;
                    composition_destination(target.id, module_type, parameter)
                }
                MappingTarget::LfoRate(target) => {
                    let source = find_source(&mut sources, target)?;
                    lfo::Settings::decode(&source.state).map_err(unsupported)?;
                    lfo_rate_destination(target)
                }
            };
            if routes
                .iter()
                .any(|route| source.owns(route) && route.destination == destination)
            {
                return Err(Invalid("this source already modulates the parameter"));
            }
            routes.push(DeviceModulationRoute {
                id: ModulationRouteId::new(project.next_identity())?,
                source: source.source,
                output: source.output,
                destination,
                amount: ModulationAmount::new(depth)?,
                range: if pressure {
                    unipolar_range()?
                } else {
                    ModulationRange::default()
                },
                enabled: true,
            });
        }
        Edit::SetDepth(id, depth) => find_route(&mut routes, id)?.amount = depth,
        Edit::Adjust(id, change) => {
            let route = find_route(&mut routes, id)?;
            route.amount = ModulationAmount::new((route.amount.get() + change).clamp(-1.0, 1.0))?;
        }
        Edit::Range(id) => {
            let route = find_route(&mut routes, id)?;
            if is_button_source(&sources, route.source) {
                return Err(Invalid("button assignments use the original on/off values"));
            }
            route.range = if route.range == ModulationRange::default() {
                unipolar_range()?
            } else {
                ModulationRange::default()
            };
        }
        Edit::ToggleRoute(id) => {
            let route = find_route(&mut routes, id)?;
            route.enabled = !route.enabled;
        }
        Edit::RemoveRoute(id) => routes.retain(|route| route.id != id),
    }
    DeviceModulation::new(
        sources.into_boxed_slice().into_vec(),
        routes.into_boxed_slice().into_vec(),
    )
}

pub(in crate::editor) fn find_source(
    sources: &mut [Modulator],
    id: ModulationSourceId,
) -> Result<&mut Modulator, Invalid> {
    sources
        .iter_mut()
        .find(|source| source.id == id)
        .ok_or(Invalid("modulator no longer exists"))
}

pub(in crate::editor) fn find_route(
    routes: &mut [DeviceModulationRoute],
    id: ModulationRouteId,
) -> Result<&mut DeviceModulationRoute, Invalid> {
    routes
        .iter_mut()
        .find(|route| route.id == id)
        .ok_or(Invalid("modulation route no longer exists"))
}
