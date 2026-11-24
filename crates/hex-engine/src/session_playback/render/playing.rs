//! Playing-transport block rendering and due-route application.
use super::*;

impl SessionPlayback {
    pub(super) fn render_playing<P: MediaPages + ?Sized>(
        &mut self,
        output: &mut [Frame],
        pages: Option<&mut P>,
        observer: &mut Option<&mut crate::NodeSignalObserver<'_>>,
        capture: &mut Option<&mut dyn CaptureSink>,
        foreign: &mut dyn hex_lattice::ForeignProcessors,
        inputs: RenderBlockInputs<'_>,
    ) -> Result<(), SessionPlaybackError> {
        // Move only the existing scratch Box, then restore it on every result.
        // No callback allocation, new owner, or final source/graph drop occurs.
        let mut scalar = self
            .native_parameter_state
            .as_deref_mut()
            .and_then(|state| state.scalar.take());
        let mut sinks = RenderSinks { observer, capture };
        let result = self.render_playing_with_scalar(
            output,
            pages,
            &mut sinks,
            foreign,
            scalar.as_deref_mut(),
            inputs,
        );
        if let Some(state) = self.native_parameter_state.as_deref_mut() {
            state.scalar = scalar;
        }
        result
    }

    fn render_playing_with_scalar<P: MediaPages + ?Sized>(
        &mut self,
        output: &mut [Frame],
        mut pages: Option<&mut P>,
        sinks: &mut RenderSinks<'_, '_, '_, '_>,
        foreign: &mut dyn hex_lattice::ForeignProcessors,
        mut scalar: Option<&mut crate::scalar_automation::StateSet>,
        inputs: RenderBlockInputs<'_>,
    ) -> Result<(), SessionPlaybackError> {
        let RenderBlockInputs {
            sources,
            live_audio,
            ..
        } = inputs;
        let observer = &mut *sinks.observer;
        let capture = &mut *sinks.capture;
        let width =
            u64::try_from(output.len()).map_err(|_| SessionPlaybackError::ArithmeticOverflow)?;
        self.performance_frame
            .checked_add(width)
            .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
        for chain in &mut self.device_chains {
            chain.begin_block();
        }
        for chain in self.source_effects.chains_mut() {
            chain.begin_block();
        }
        for chain in &mut self.return_chains {
            chain.begin_block();
        }
        self.main_chain.begin_block();
        self.return_effect_tails.begin_block();
        for runtime in self.mutable_instruments.iter_mut().flatten() {
            runtime.begin_block();
        }
        for tail in self.tail_mutable_instruments.iter_mut().flatten() {
            tail.begin_block();
        }
        for chain in self.tail_groups.iter_mut().flatten() {
            chain.begin_block();
        }
        self.tail_output_scratch
            .copy_from_slice(&self.tail_output_active);
        self.route_scratch.copy_from_slice(&self.routes);
        self.release_scratch.clear();
        self.release_scratch.extend_from_slice(&self.releases);
        self.release_order_scratch = self.release_order;
        self.release_scratch
            .retain(|entry| entry.end_frame() > self.performance_frame);
        self.suppression_scratch.clear();
        self.suppression_scratch.extend(
            self.suppressions
                .iter()
                .copied()
                .filter(|entry| entry.until_frame > self.performance_frame),
        );
        let mut performance_frame = self.performance_frame;
        let mut arrangement_frame = self.arrangement_frame;
        let mut arrangement_loop_pass = self.arrangement_loop_pass;
        let mut output_offset = 0;
        let mut timing_run = capture
            .as_ref()
            .filter(|sink| sink.observes_timing())
            .map(|_| (0, arrangement_frame));
        let mut route_changed = false;
        let region = self.active_loop_region();
        // Pending route frames only change when apply_due_routes consumes them.
        // Keep the next deadline for this render transaction so compensated
        // graphs do not rescan every track several times for every sample.
        let mut next_route = next_pending_frame(&self.route_scratch);
        let mut resident_source_index = 0;
        let mut resident_source_end = 0;
        let mut resident_source_eligible = [false; MAX_TRACKS];
        let mut resident_instrument_eligible = [false; MAX_TRACKS];
        let mut aligned = [false; MAX_TRACKS];
        let mut passthrough = [compensated::ResidentRoute::Scalar; MAX_TRACKS];
        let mut stamps = [None; 256];
        let mut aligned_main = [[0.0; 2]; 256];
        let mut main_is_batched = false;
        let mut projected_history = None;
        let mut batched_sends = false;
        // Shared-source contributions can cross track ownership; retain the
        // scalar route for that graph until its live-input batching is proven.
        let live_source_batch_safe = live_audio.is_none_or(|input| {
            self.route_scratch
                .iter()
                .position(|route| route.id == input.track)
                .is_some_and(|index| {
                    !self.revision.source_routing().is_shared(index)
                        && !self.revision.source_routing().contributes(index)
                })
        });

        while output_offset < output.len() {
            if let Some(region) = region {
                (arrangement_frame, arrangement_loop_pass) =
                    normalize_loop_cursor(arrangement_frame, region, arrangement_loop_pass)?;
            }
            self.browser_audition.voices.settle(
                performance_frame,
                arrangement_frame,
                arrangement_loop_pass,
            );
            if let Some((offset, start)) = timing_run
                && start.checked_add((output_offset - offset) as u64) != Some(arrangement_frame)
            {
                if output_offset > offset
                    && let Some(sink) = capture.as_mut()
                {
                    sink.timing(
                        offset,
                        start,
                        output_offset - offset,
                        self.revision.session().timing(),
                    );
                }
                timing_run = Some((output_offset, arrangement_frame));
            }
            let (captured_release, applied_route) = if self.current_sampler_console
                || next_route.is_some_and(|next| next <= performance_frame)
            {
                let result = self.apply_due_routes(
                    performance_frame,
                    arrangement_frame,
                    (arrangement_loop_pass == 0)
                        .then_some(self.interactive_onset_floor)
                        .flatten(),
                )?;
                next_route = next_pending_frame(&self.route_scratch);
                result
            } else {
                (false, false)
            };
            route_changed |= applied_route;
            if captured_release {
                self.release_scratch.sort_unstable_by_key(|entry| {
                    (
                        entry.location.track_index(),
                        entry.location.tail_group(),
                        entry.order,
                    )
                });
            }
            let upstream_frame_routing = self
                .compensation
                .as_ref()
                .is_some_and(|runtime| runtime.graph.needs_upstream_frame_routing());
            if upstream_frame_routing
                && resident_source_index == resident_source_end
                && scalar.is_none()
            {
                let mut block_count = (output.len() - output_offset).min(256);
                if let Some(next) = next_route.filter(|next| *next > performance_frame) {
                    block_count = block_count
                        .min(usize::try_from(next - performance_frame).unwrap_or(usize::MAX));
                }
                if let Some(region) = region {
                    let until_wrap = if arrangement_frame < region.start() {
                        region.start() - arrangement_frame
                    } else {
                        region.end() - arrangement_frame
                    };
                    block_count =
                        block_count.min(usize::try_from(until_wrap).unwrap_or(usize::MAX));
                }
                if let Some(keyboard) = self.computer_keyboard.as_ref() {
                    block_count = keyboard.source_input_span(block_count);
                }
                if self.can_block_compensated_route(block_count) {
                    let chunk_end = output_offset
                        .checked_add(block_count)
                        .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
                    let mut tail_scratch = std::mem::take(&mut self.grouped_tail_scratch);
                    let rendered = self.render_compensated_route_block(
                        performance_frame,
                        arrangement_frame,
                        arrangement_loop_pass,
                        &mut output[output_offset..chunk_end],
                        pages.as_deref_mut(),
                        output_offset,
                        observer,
                        capture,
                        inputs,
                        foreign,
                        &mut tail_scratch,
                    );
                    self.grouped_tail_scratch = tail_scratch;
                    rendered?;
                    let chunk = u64::try_from(block_count)
                        .map_err(|_| SessionPlaybackError::ArithmeticOverflow)?;
                    performance_frame = performance_frame
                        .checked_add(chunk)
                        .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
                    arrangement_frame = arrangement_frame
                        .checked_add(chunk)
                        .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
                    if let Some(region) = region {
                        (arrangement_frame, arrangement_loop_pass) = normalize_loop_cursor(
                            arrangement_frame,
                            region,
                            arrangement_loop_pass,
                        )?;
                    }
                    self.release_scratch
                        .retain(|entry| entry.end_frame() > performance_frame);
                    output_offset = chunk_end;
                    continue;
                }
            }
            let use_resident_sources = upstream_frame_routing
                && self.playing
                && !self.resident_source_scratch.is_empty()
                && self.computer_keyboard.is_none()
                && self.current_sampler_keyboards.is_empty()
                && !self.current_sampler_console
                && live_source_batch_safe;
            if use_resident_sources && resident_source_index == resident_source_end {
                let mut count = (output.len() - output_offset).min(256);
                if let Some(next) = next_route.filter(|next| *next > performance_frame) {
                    count =
                        count.min(usize::try_from(next - performance_frame).unwrap_or(usize::MAX));
                }
                if let Some(region) = region {
                    let until = if arrangement_frame < region.start() {
                        region.start() - arrangement_frame
                    } else {
                        region.end() - arrangement_frame
                    };
                    count = count.min(usize::try_from(until).unwrap_or(usize::MAX));
                }
                if count == 0 {
                    return Err(SessionPlaybackError::ArithmeticOverflow);
                }
                let mutable_loop_pass = (count > 1
                    && self.revision.session().has_mutable_instruments()
                    && observer.is_none()
                    && capture.is_none()
                    && pages.is_none()
                    && sources.is_none()
                    && scalar.is_none()
                    && live_audio.is_none()
                    && self.release_scratch.is_empty()
                    && self
                        .native_parameter_state
                        .as_deref()
                        .is_none_or(|state| state.processing.is_none())
                    && self
                        .compensation
                        .as_ref()
                        .is_some_and(|runtime| runtime.graph.independent_main_routes()))
                .then_some(arrangement_loop_pass);
                self.prepare_resident_sources(
                    compensated::ResidentSourceWindow {
                        performance_frame,
                        arrangement_frame,
                        frames: count,
                        mutable_loop_pass,
                        interactive_onset_floor: (arrangement_loop_pass == 0)
                            .then_some(self.interactive_onset_floor)
                            .flatten(),
                        chase_at_start: self.interactive_start_frame == Some(performance_frame),
                    },
                    &mut resident_source_eligible,
                    &mut resident_instrument_eligible,
                    live_audio.map(|input| input.track),
                )?;
                projected_history = if observer.is_none() && scalar.is_none() {
                    self.prepare_native_origin_window(
                        performance_frame,
                        arrangement_frame,
                        arrangement_loop_pass,
                        &mut stamps[..count],
                    )?
                } else {
                    None
                };
                self.align_resident_tracks(
                    arrangement_frame,
                    count,
                    &resident_source_eligible,
                    projected_history.map(|frames| (&stamps[..count], frames)),
                    &mut aligned,
                )?;
                passthrough.fill(compensated::ResidentRoute::Scalar);
                if observer.is_none()
                    && scalar.is_none()
                    && self
                        .native_parameter_state
                        .as_deref()
                        .is_none_or(|state| state.processing.is_none())
                    && !self.tail_output_scratch.iter().any(|active| *active)
                    && !self
                        .return_effect_tails
                        .slots()
                        .iter()
                        .flatten()
                        .any(|tail| tail.output_active)
                {
                    self.classify_resident_return_tracks(
                        &resident_source_eligible,
                        &mut passthrough,
                    );
                }
                self.prepare_unity_faders(count, &mut passthrough, &aligned);
                batched_sends = self.prepare_resident_return_sends(count, &passthrough);
                // Capture still needs individual per-frame track taps.
                // A fully aligned flat graph, or the independent Return
                // send path, can sum track PCM in Main order ahead of time.
                main_is_batched = count > 1
                    && capture.is_none()
                    && !self.route_scratch.is_empty()
                    && (batched_sends
                        || aligned[..self.route_scratch.len()]
                            .iter()
                            .all(|ready| *ready));
                if main_is_batched {
                    let main = &mut aligned_main[..count];
                    main.copy_from_slice(&output[output_offset..output_offset + count]);
                    if batched_sends {
                        self.prepare_resident_return_main(main, &passthrough)?;
                    } else {
                        for track in &self.resident_source_scratch {
                            compensated::add_main_track(main, track);
                        }
                    }
                }
                resident_source_index = 0;
                resident_source_end = count;
            }
            let mut chunk_frames =
                (output.len() - output_offset).min(if upstream_frame_routing { 1 } else { 256 });
            if let Some(keyboard) = self.computer_keyboard.as_ref() {
                chunk_frames = keyboard.source_input_span(chunk_frames);
            }
            let current_first_frame = self.current_sampler_console
                && self.route_scratch.iter().any(|r| r.current_first_frame);
            if current_first_frame {
                chunk_frames = chunk_frames.min(1);
            }
            if let Some(next) = next_route
                && next > performance_frame
            {
                let until = usize::try_from(next - performance_frame).unwrap_or(usize::MAX);
                chunk_frames = chunk_frames.min(until);
            }
            if let Some(region) = region {
                let until_wrap = if arrangement_frame < region.start() {
                    region.start() - arrangement_frame
                } else {
                    region.end() - arrangement_frame
                };
                chunk_frames = chunk_frames.min(usize::try_from(until_wrap).unwrap_or(usize::MAX));
            }
            if chunk_frames == 0 {
                if let Some(region) = region {
                    (arrangement_frame, arrangement_loop_pass) =
                        normalize_loop_cursor(arrangement_frame, region, arrangement_loop_pass)?;
                    continue;
                }
                return Err(SessionPlaybackError::ArithmeticOverflow);
            }
            let chunk_end = output_offset
                .checked_add(chunk_frames)
                .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
            if let Some(scalar) = scalar.as_deref_mut() {
                let bindings = self
                    .revision
                    .scalar_automation
                    .as_deref()
                    .ok_or(SessionPlaybackError::DeviceRuntime)?;
                let endpoint = pages
                    .as_deref_mut()
                    .ok_or(SessionPlaybackError::MediaPagesUnavailable)?;
                let history = self.compensation.as_ref().map(|runtime| &runtime.delays);
                let origin = |source_offset| {
                    arrangement_frame
                        .checked_add(source_offset)
                        .map(Some)
                        .ok_or(crate::Invalid("scalar automation origin overflow"))
                };
                scalar
                    .fill(
                        bindings,
                        endpoint,
                        chunk_frames,
                        output_offset,
                        |delay, index| {
                            if delay == 0 {
                                origin(index as u64)
                            } else {
                                crate::compensation::CompensationRuntime::preview_stage_frame(
                                    history,
                                    delay,
                                    index as u64,
                                    origin,
                                )
                            }
                        },
                    )
                    .map_err(|_| SessionPlaybackError::MediaPagesUnavailable)?;
            }
            let external = scalar
                .as_deref()
                .map(|state| state as &dyn hex_lattice::NativeExternalAutomationSet);
            if upstream_frame_routing {
                self.render_compensated_route_frame(
                    performance_frame,
                    arrangement_frame,
                    arrangement_loop_pass,
                    &mut output[output_offset..chunk_end],
                    pages.as_deref_mut(),
                    output_offset,
                    observer,
                    capture,
                    sources,
                    foreign,
                    external,
                    use_resident_sources.then(|| compensated::BatchedSources {
                        index: resident_source_index,
                        audio: &resident_source_eligible,
                        instrument: &resident_instrument_eligible,
                        stamps: projected_history.map(|_| &stamps[..resident_source_end]),
                        aligned: &aligned,
                        passthrough: &passthrough,
                        batched_sends,
                        main: main_is_batched.then(|| aligned_main[resident_source_index]),
                    }),
                    live_audio,
                )?;
                if use_resident_sources {
                    resident_source_index += 1;
                }
            } else {
                self.render_route_chunk(
                    &mut output[output_offset..chunk_end],
                    pages.as_deref_mut(),
                    RouteChunkTiming {
                        performance_frame,
                        arrangement_frame,
                        arrangement_loop_pass,
                        frame_offset: output_offset,
                    },
                    inputs,
                    &mut RenderSinks { observer, capture },
                    ForeignRenderStage { foreign, external },
                )?;
            }
            if current_first_frame {
                for route in &mut self.route_scratch {
                    route.current_first_frame = false;
                }
            }
            if !upstream_frame_routing
                && chunk_frames > 1
                && self.compensation.as_ref().is_some_and(|runtime| {
                    runtime.delays.running_stamp_streak() >= runtime.horizon()
                })
            {
                let window = &mut stamps[..chunk_frames];
                self.fill_compensated_stamps(
                    performance_frame,
                    arrangement_frame,
                    arrangement_loop_pass,
                    window,
                )?;
                self.render_compensated_main_block(
                    &mut output[output_offset..chunk_end],
                    output_offset,
                    window,
                    &mut [[0.0; 2]; 256],
                    foreign,
                    observer,
                    external,
                )?;
            } else if self.compensation.is_some() && !upstream_frame_routing {
                // During pre-roll each stage must silence frames whose content
                // origin has not yet arrived. Upstream processing stays batched.
                for index in output_offset..chunk_end {
                    let advance = (index - output_offset) as u64;
                    let performance = performance_frame
                        .checked_add(advance)
                        .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
                    let arrangement = arrangement_frame
                        .checked_add(advance)
                        .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
                    self.push_compensated_stamp(performance, arrangement, arrangement_loop_pass)?;
                    self.compensation
                        .as_mut()
                        .ok_or(SessionPlaybackError::DeviceRuntime)?
                        .process_main_with_foreign(
                            &mut self.main_chain,
                            &mut output[index..index + 1],
                            index,
                            foreign,
                            observer,
                            self.native_parameter_state
                                .as_deref_mut()
                                .and_then(|state| state.processing.as_mut()),
                            external,
                        )
                        .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
                    let clock = crate::effect_clock::ArrangementEffectClock::session(
                        self.revision.session().timing(),
                        arrangement,
                        performance,
                        arrangement_loop_pass,
                        true,
                    );
                    self.return_effect_tails.render_main_chunk(
                        &mut output[index..index + 1],
                        MainTailClock::current(self.compensation.as_ref(), &clock),
                        index,
                        foreign,
                    )?;
                }
            } else {
                if let Some(compensation) = self.compensation.as_mut() {
                    compensation
                        .process_main_with_foreign(
                            &mut self.main_chain,
                            &mut output[output_offset..chunk_end],
                            output_offset,
                            foreign,
                            observer,
                            self.native_parameter_state
                                .as_deref_mut()
                                .and_then(|state| state.processing.as_mut()),
                            external,
                        )
                        .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
                } else {
                    let main_clock = crate::effect_clock::ArrangementEffectClock::session(
                        self.revision.session().timing(),
                        arrangement_frame,
                        performance_frame,
                        arrangement_loop_pass,
                        true,
                    );
                    crate::main_chain::process_with_native_input_and_external(
                        &mut self.main_chain,
                        &mut output[output_offset..chunk_end],
                        output_offset,
                        Some(&main_clock),
                        self.native_parameter_state
                            .as_deref_mut()
                            .and_then(|state| state.processing.as_mut()),
                        external,
                        observer,
                    )
                    .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
                }
                let tail_clock = crate::effect_clock::ArrangementEffectClock::session(
                    self.revision.session().timing(),
                    arrangement_frame,
                    performance_frame,
                    arrangement_loop_pass,
                    true,
                );
                self.return_effect_tails.render_main_chunk(
                    &mut output[output_offset..chunk_end],
                    MainTailClock::current(self.compensation.as_ref(), &tail_clock),
                    output_offset,
                    foreign,
                )?;
            }
            let chunk = u64::try_from(chunk_frames)
                .map_err(|_| SessionPlaybackError::ArithmeticOverflow)?;
            performance_frame = performance_frame
                .checked_add(chunk)
                .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
            arrangement_frame = arrangement_frame
                .checked_add(chunk)
                .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
            if let Some(region) = region {
                (arrangement_frame, arrangement_loop_pass) =
                    normalize_loop_cursor(arrangement_frame, region, arrangement_loop_pass)?;
            }
            self.release_scratch
                .retain(|entry| entry.end_frame() > performance_frame);
            for route in &mut self.route_scratch {
                route.audio_release = route
                    .audio_release
                    .filter(|release| release.active_at(performance_frame));
            }
            self.suppression_scratch
                .retain(|entry| entry.until_frame > performance_frame);
            output_offset = chunk_end;
        }

        if let Some((offset, start)) = timing_run
            && let Some(sink) = capture.as_mut()
        {
            sink.timing(
                offset,
                start,
                output_offset - offset,
                self.revision.session().timing(),
            );
        }
        self.main_chain.commit_block();
        if let Some(delay) = self.main_pass_delay.as_mut() {
            delay.process(output);
        }
        self.revision.arrangement().master_mix().apply(output);
        for chain in &mut self.device_chains {
            chain.commit_block();
        }
        for chain in self.source_effects.chains_mut() {
            chain.commit_block();
        }
        for chain in &mut self.return_chains {
            chain.commit_block();
        }
        self.return_effect_tails.commit_block();
        for runtime in self.mutable_instruments.iter_mut().flatten() {
            runtime.commit_block();
        }
        for tail in self.tail_mutable_instruments.iter_mut().flatten() {
            tail.commit_block();
        }
        for chain in self.tail_groups.iter_mut().flatten() {
            chain.commit_block();
        }
        self.tail_output_active
            .copy_from_slice(&self.tail_output_scratch);
        std::mem::swap(&mut self.routes, &mut self.route_scratch);
        std::mem::swap(&mut self.releases, &mut self.release_scratch);
        std::mem::swap(&mut self.suppressions, &mut self.suppression_scratch);
        self.release_order = self.release_order_scratch;
        self.performance_frame = performance_frame;
        self.arrangement_frame = arrangement_frame;
        self.arrangement_loop_pass = arrangement_loop_pass;
        self.interactive_start_frame = None;
        self.pending_shared_chase = None;
        self.pending_shared_live = None;
        if route_changed {
            self.advance_route_state_generation();
        }
        Ok(())
    }

    fn apply_due_routes(
        &mut self,
        performance_frame: u64,
        arrangement_frame: u64,
        interactive_onset_floor: Option<crate::midi_compile::InteractiveOnsetFloor>,
    ) -> Result<(bool, bool), SessionPlaybackError> {
        let mut captured_release = false;
        let mut applied_route = false;
        self.preflight_current_due_routes(performance_frame, arrangement_frame)?;
        self.preflight_shared_due_routes(performance_frame)?;
        // Validate every shared restart identity before cutting any speculative
        // gate. Selected MIDI roots may own several independent destinations.
        for (index, route) in self.route_scratch.iter().enumerate() {
            if route
                .pending
                .is_none_or(|pending| pending.frame > performance_frame)
            {
                continue;
            }
            for destination in 0..self.route_scratch.len() {
                if self.revision.source_routing().is_shared(destination)
                    && self.revision.arrangement().tracks()[destination]
                        .midi_source_index
                        .unwrap_or(destination)
                        == index
                {
                    self.route_scratch[destination]
                        .current_incarnation
                        .checked_add(1)
                        .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
                }
            }
        }

        for index in 0..self.route_scratch.len() {
            let Some(pending) = self.route_scratch[index]
                .pending
                .filter(|pending| pending.frame <= performance_frame)
            else {
                continue;
            };
            let owner = self.route_scratch[index].owner;
            let current_plan = self.current_route_cut_scratch.get(index).copied().flatten();
            if self.current_sampler_route_track(index) {
                let plan = current_plan.ok_or(SessionPlaybackError::InvalidCommand)?;
                if plan.current_sampler_cut {
                    self.mutable_instruments
                        .get_mut(index)
                        .and_then(Option::as_mut)
                        .ok_or(SessionPlaybackError::MutableInstrumentRuntime)?
                        .cut_speculative_gate_deadlines_at(plan.frame);
                    self.route_scratch[index].current_incarnation = plan.incarnation;
                    self.route_scratch[index].current_first_frame = true;
                }
                self.route_scratch[index].owner = plan.owner;
                self.route_scratch[index].pending = None;
                let fade = self.audio_release_fade();
                super::audio_release::begin(
                    &mut self.route_scratch[index],
                    owner,
                    performance_frame,
                    fade,
                )?;
                let track = self.route_scratch[index].id;
                self.suppression_scratch.retain(|e| e.track != track);
                applied_route = true;
                continue;
            }
            for destination in 0..self.route_scratch.len() {
                if self.revision.arrangement().tracks()[destination]
                    .midi_source_index
                    .unwrap_or(destination)
                    != index
                {
                    continue;
                }
                if self.revision.source_routing().is_shared(destination) {
                    self.route_scratch[destination].current_incarnation = self.route_scratch
                        [destination]
                        .current_incarnation
                        .checked_add(1)
                        .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
                    if let Some(receiver) =
                        self.revision.source_routing().receiver_index(destination)
                        && let Some(runtime) = self
                            .mutable_instruments
                            .get_mut(receiver)
                            .and_then(Option::as_mut)
                    {
                        runtime.cut_contributor_speculative_at(
                            self.route_scratch[destination].id,
                            performance_frame,
                        );
                        continue;
                    }
                }
                if self
                    .mutable_instruments
                    .get(destination)
                    .and_then(Option::as_ref)
                    .is_some_and(|runtime| !runtime.speculative_is_quiescent())
                {
                    return Err(SessionPlaybackError::MutableInstrumentActive);
                }
                if transition_captures(owner, pending.target) {
                    self.capture_outgoing(
                        destination,
                        owner,
                        arrangement_frame,
                        performance_frame,
                        interactive_onset_floor,
                    )?;
                    captured_release = true;
                }
            }
            let is_audio = matches!(pending.target, RouteTarget::Session(clip) if self.revision.session().audio_clip(clip).is_some());
            let target_beat = if matches!(pending.target, RouteTarget::Session(_)) && !is_audio {
                timing_view_for(self.revision.session(), self.periodic_tempo(), self.anchor)
                    .frame_to_beat(performance_frame)
                    .map_err(|_| SessionPlaybackError::TempoConversion)?
            } else {
                pending.beat
            };
            self.route_scratch[index].owner = if let Some(plan) = current_plan {
                plan.owner
            } else {
                self.owner_for_target(
                    pending.target,
                    target_beat,
                    performance_frame,
                    pending.note_chase,
                )?
            };
            self.route_scratch[index].pending = None;
            let fade = self.audio_release_fade();
            super::audio_release::begin(
                &mut self.route_scratch[index],
                owner,
                performance_frame,
                fade,
            )?;
            applied_route = true;
            if transition_captures(owner, pending.target) {
                for destination in 0..self.route_scratch.len() {
                    if self.revision.arrangement().tracks()[destination]
                        .midi_source_index
                        .unwrap_or(destination)
                        == index
                    {
                        let track = self.route_scratch[destination].id;
                        self.suppression_scratch
                            .retain(|entry| entry.track != track);
                    }
                }
            }
        }
        Ok((captured_release, applied_route))
    }
}
