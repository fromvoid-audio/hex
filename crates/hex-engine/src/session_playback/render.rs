mod compensated;
mod computer_keyboard;
use super::{commands::transition_captures, *};
use crate::compile::PreparedAudioSource;
use crate::{MediaPages, MidiTiming, StreamedPageSpan, render_streamed_page_span};
use hex_project::LoopRegion;
use std::sync::Arc;

impl SessionPlayback {
    /// Render a complete block transactionally. A refusal after any chunk
    /// silences the requested block and leaves committed route/clock state as-is.
    pub fn render(&mut self, output: &mut [Frame]) -> Result<(), SessionPlaybackError> {
        self.render_with_post_node_observer(output, None)
    }

    /// Render transactionally while optionally observing stable device IDs at
    /// their post-node output. Observations are speculative until this method
    /// returns `Ok`; callers must publish them only after their final output
    /// validation also succeeds.
    pub fn render_with_post_node_observer(
        &mut self,
        output: &mut [Frame],
        observer: Option<&mut crate::PostNodeObserver<'_>>,
    ) -> Result<(), SessionPlaybackError> {
        if let Some(observer) = observer {
            self.render_with_node_signal_observer(
                output,
                Some(&mut |identity, phase, offset, frames| {
                    if phase == crate::NodeSignalPhase::Output {
                        observer(identity, offset, frames);
                    }
                }),
            )
        } else {
            self.render_with_node_signal_observer(output, None)
        }
    }

    /// Render with synchronous input/output taps around each whole native entry.
    /// Input precedes host input trim; output follows effect and output trim.
    /// Observations commit only after render and final caller validation succeed.
    pub fn render_with_node_signal_observer(
        &mut self,
        output: &mut [Frame],
        observer: Option<&mut crate::NodeSignalObserver<'_>>,
    ) -> Result<(), SessionPlaybackError> {
        self.candidate_admission_open = false;
        let contains_streamed_sources = if self.linear_arrangement_export {
            self.revision.arrangement().contains_streamed_sources()
        } else {
            self.revision.contains_streamed_sources()
        };
        if self.playing && !output.is_empty() && contains_streamed_sources {
            return Err(SessionPlaybackError::MediaPagesUnavailable);
        }
        self.render_internal::<dyn MediaPages>(output, None, observer, None)
    }

    /// Render with a borrowed page view. Misses silence only the unavailable
    /// source span; other tracks and the performance clock continue.
    pub fn render_with_pages<P: MediaPages>(
        &mut self,
        output: &mut [Frame],
        pages: &mut P,
    ) -> Result<(), SessionPlaybackError> {
        self.candidate_admission_open = false;
        self.render_internal(output, Some(pages), None, None)
    }

    /// Capture selected canonical outputs from the same complete linear pass.
    /// Preflight precedes runtime mutation; buffers are invalid on every error.
    pub fn render_with_capture(
        &mut self,
        output: &mut [Frame],
        capture: &mut OutputCapture<'_, '_>,
    ) -> Result<(), SessionPlaybackError> {
        self.render_captured::<dyn MediaPages>(output, None, capture)
    }

    /// Streamed counterpart of `render_with_capture`; page misses keep their
    /// existing source-local silence policy. No second source render occurs.
    pub fn render_with_pages_and_capture<P: MediaPages>(
        &mut self,
        output: &mut [Frame],
        pages: &mut P,
        capture: &mut OutputCapture<'_, '_>,
    ) -> Result<(), SessionPlaybackError> {
        self.render_captured(output, Some(pages), capture)
    }

    fn render_captured<P: MediaPages + ?Sized>(
        &mut self,
        output: &mut [Frame],
        pages: Option<&mut P>,
        capture: &mut OutputCapture<'_, '_>,
    ) -> Result<(), SessionPlaybackError> {
        if let Err(error) = capture.validate(self, output.len()) {
            output.fill([0.0; 2]);
            capture.invalidate();
            return Err(SessionPlaybackError::Capture(error));
        }
        if pages.is_none()
            && self.playing
            && !output.is_empty()
            && self.revision.arrangement().contains_streamed_sources()
        {
            output.fill([0.0; 2]);
            capture.invalidate();
            return Err(SessionPlaybackError::MediaPagesUnavailable);
        }
        capture.begin(output.len());
        self.candidate_admission_open = false;
        self.render_internal(output, pages, None, Some(capture))
    }

    /// Render with borrowed pages and optional borrowed post-node observation.
    /// Neither borrow is retained. Observers must not allocate, block, clone
    /// owners or retain sample slices. Samples are speculative: commit them
    /// only after this block and the caller's final output validation succeed.
    pub fn render_with_pages_and_post_node_observer<P: MediaPages + ?Sized>(
        &mut self,
        output: &mut [Frame],
        pages: &mut P,
        observer: Option<&mut crate::PostNodeObserver<'_>>,
    ) -> Result<(), SessionPlaybackError> {
        if let Some(observer) = observer {
            self.render_with_pages_and_node_signal_observer(
                output,
                pages,
                Some(&mut |identity, phase, offset, frames| {
                    if phase == crate::NodeSignalPhase::Output {
                        observer(identity, offset, frames);
                    }
                }),
            )
        } else {
            self.render_with_pages_and_node_signal_observer(output, pages, None)
        }
    }

    /// Render with synchronous input/output taps around each whole native entry.
    /// Input precedes host input trim; output follows effect and output trim.
    /// Observations commit only after render and final caller validation succeed.
    pub fn render_with_pages_and_node_signal_observer<P: MediaPages + ?Sized>(
        &mut self,
        output: &mut [Frame],
        pages: &mut P,
        observer: Option<&mut crate::NodeSignalObserver<'_>>,
    ) -> Result<(), SessionPlaybackError> {
        self.candidate_admission_open = false;
        self.render_internal(output, Some(pages), observer, None)
    }

    /// Observe stereo Main after the existing full render/master/commit path.
    /// Device observations may be speculative during rendering; Main is called
    /// once only after success. Neither is an accepted native callback window.
    /// The caller owns final output validation and must not publish early.
    pub fn render_with_post_node_and_main_observers(
        &mut self,
        output: &mut [Frame],
        post_node: Option<&mut crate::PostNodeObserver<'_>>,
        main: Option<&mut crate::MainOutputObserver<'_>>,
    ) -> Result<(), SessionPlaybackError> {
        self.render_with_post_node_observer(output, post_node)?;
        if let Some(main) = main {
            main(output);
        }
        Ok(())
    }

    /// Borrowed-page counterpart. No second pass, persistent observer state or
    /// callback-window/identity/rollback guarantee is introduced. Real-time
    /// observers must not allocate, block, panic or retain their sample borrow.
    pub fn render_with_pages_post_node_and_main_observers<P: MediaPages + ?Sized>(
        &mut self,
        output: &mut [Frame],
        pages: &mut P,
        post_node: Option<&mut crate::PostNodeObserver<'_>>,
        main: Option<&mut crate::MainOutputObserver<'_>>,
    ) -> Result<(), SessionPlaybackError> {
        self.render_with_pages_and_post_node_observer(output, pages, post_node)?;
        if let Some(main) = main {
            main(output);
        }
        Ok(())
    }

    fn render_internal<P: MediaPages + ?Sized>(
        &mut self,
        output: &mut [Frame],
        pages: Option<&mut P>,
        observer: Option<&mut crate::NodeSignalObserver<'_>>,
        mut capture: Option<&mut OutputCapture<'_, '_>>,
    ) -> Result<(), SessionPlaybackError> {
        output.fill([0.0; 2]);
        if output.is_empty() || (!self.playing && self.computer_keyboard.is_none()) {
            if let Some(capture) = capture.as_mut() {
                capture.commit(output.len());
            }
            return Ok(());
        }
        let selected = self
            .live_expression
            .as_ref()
            .map(|input| (input.track, input.instrument, input.snapshot()));
        for (route, runtime) in self.routes.iter().zip(self.mutable_instruments.iter_mut()) {
            if let Some(runtime) = runtime {
                runtime.expression = match selected {
                    Some((track, instrument, snapshot))
                        if track == route.id && runtime.expression_identity == Some(instrument) =>
                    {
                        Some(snapshot)
                    }
                    _ => runtime
                        .expression
                        .map(|_| crate::live_expression::Snapshot::default()),
                };
            }
        }
        for tail in self.tail_mutable_instruments.iter_mut().flatten() {
            tail.runtime.expression = match selected {
                Some((track, instrument, snapshot))
                    if track == tail.track_id
                        && tail.runtime.expression_identity == Some(instrument) =>
                {
                    Some(snapshot)
                }
                _ => tail
                    .runtime
                    .expression
                    .map(|_| crate::live_expression::Snapshot::default()),
            };
        }
        if let Some(keyboard) = self.computer_keyboard.as_mut() {
            keyboard.set_expression(selected);
        }
        let mut observer = observer;
        if let Some(compensation) = self.compensation.as_mut() {
            compensation
                .delays
                .begin_transaction()
                .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
        }
        if let Some(keyboard) = &mut self.computer_keyboard {
            keyboard.begin_block();
        }
        let result = if self.playing {
            self.render_playing(output, pages, &mut observer, &mut capture)
        } else {
            self.render_keyboard_monitor(output, &mut observer, &mut capture)
        };
        if result.is_err() {
            for chain in self
                .device_chains
                .iter_mut()
                .chain(self.return_chains.iter_mut())
            {
                chain.rollback_block();
            }
            for chain in self.tail_groups.iter_mut().flatten() {
                chain.rollback_block();
            }
            if let Some(keyboard) = &mut self.computer_keyboard {
                keyboard.rollback();
            }
            if let Some(compensation) = self.compensation.as_mut() {
                let _ = compensation.delays.rollback_transaction();
            }
            output.fill([0.0; 2]);
            for runtime in self.mutable_instruments.iter_mut().flatten() {
                runtime.rollback_block();
            }
            for tail in self.tail_mutable_instruments.iter_mut().flatten() {
                tail.runtime.rollback_block();
            }
            self.return_effect_tails.rollback_block();
            if let Some(capture) = capture.as_mut() {
                capture.invalidate();
            }
        } else {
            if let Some(keyboard) = &mut self.computer_keyboard {
                keyboard.commit();
            }
            if let Some(compensation) = self.compensation.as_mut() {
                compensation
                    .delays
                    .commit_transaction()
                    .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
            }
            // The ordinary current context already owns the target endpoint.
            // Retire only after the whole block commits; detached voices retain
            // their own captured transition, and refused blocks keep this state.
            for transition in &mut self.analytic_continuations {
                if transition.as_ref().is_some_and(|transition| {
                    transition
                        .elapsed_at(self.performance_frame)
                        .is_ok_and(|elapsed| elapsed == transition.duration())
                }) {
                    *transition = None;
                }
            }
            if let Some(capture) = capture.as_mut() {
                capture.commit(output.len());
            }
        }
        result
    }

    fn render_playing<P: MediaPages + ?Sized>(
        &mut self,
        output: &mut [Frame],
        mut pages: Option<&mut P>,
        observer: &mut Option<&mut crate::NodeSignalObserver<'_>>,
        capture: &mut Option<&mut OutputCapture<'_, '_>>,
    ) -> Result<(), SessionPlaybackError> {
        let width =
            u64::try_from(output.len()).map_err(|_| SessionPlaybackError::ArithmeticOverflow)?;
        self.performance_frame
            .checked_add(width)
            .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
        for chain in &mut self.device_chains {
            chain.begin_block();
        }
        for chain in &mut self.return_chains {
            chain.begin_block();
        }
        self.return_effect_tails.begin_block();
        for runtime in self.mutable_instruments.iter_mut().flatten() {
            runtime.begin_block();
        }
        for tail in self.tail_mutable_instruments.iter_mut().flatten() {
            tail.runtime.begin_block();
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
            .retain(|entry| entry.voice.end_frame() > self.performance_frame);
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
        let mut route_changed = false;
        let region = (!self.linear_arrangement_export)
            .then(|| self.revision.arrangement().loop_region())
            .flatten()
            .filter(|loop_region| loop_region.enabled());

        while output_offset < output.len() {
            if let Some(region) = region {
                (arrangement_frame, arrangement_loop_pass) =
                    normalize_loop_cursor(arrangement_frame, region, arrangement_loop_pass)?;
            }
            let (captured_release, applied_route) =
                self.apply_due_routes(performance_frame, arrangement_frame)?;
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
            let mut chunk_frames = (output.len() - output_offset)
                .min(if self.compensation.is_some() { 1 } else { 256 });
            if let Some(next) = next_pending_frame(&self.route_scratch) {
                if next > performance_frame {
                    let until = usize::try_from(next - performance_frame).unwrap_or(usize::MAX);
                    chunk_frames = chunk_frames.min(until);
                }
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
            if self.compensation.is_some() {
                self.render_compensated_route_frame(
                    performance_frame,
                    arrangement_frame,
                    arrangement_loop_pass,
                    &mut output[output_offset..chunk_end],
                    pages.as_deref_mut(),
                    output_offset,
                    observer,
                    capture,
                )?;
            } else {
                self.render_route_chunk(
                    performance_frame,
                    arrangement_frame,
                    arrangement_loop_pass,
                    &mut output[output_offset..chunk_end],
                    pages.as_deref_mut(),
                    output_offset,
                    observer,
                    capture,
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
                .retain(|entry| entry.voice.end_frame() > performance_frame);
            self.suppression_scratch
                .retain(|entry| entry.until_frame > performance_frame);
            output_offset = chunk_end;
        }

        self.revision.arrangement().master_mix().apply(output);
        for chain in &mut self.device_chains {
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
            tail.runtime.commit_block();
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
        if route_changed {
            self.advance_route_state_generation();
        }
        Ok(())
    }

    fn apply_due_routes(
        &mut self,
        performance_frame: u64,
        arrangement_frame: u64,
    ) -> Result<(bool, bool), SessionPlaybackError> {
        let mut captured_release = false;
        let mut applied_route = false;
        for index in 0..self.route_scratch.len() {
            let Some(pending) = self.route_scratch[index]
                .pending
                .filter(|pending| pending.frame <= performance_frame)
            else {
                continue;
            };
            let owner = self.route_scratch[index].owner;
            for destination in 0..self.route_scratch.len() {
                if self.revision.arrangement().tracks()[destination]
                    .midi_source_index
                    .unwrap_or(destination)
                    != index
                {
                    continue;
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
                    )?;
                    captured_release = true;
                }
            }
            let is_audio = matches!(pending.target, RouteTarget::Session(clip) if self.revision.session().audio_clip(clip).is_some());
            let target_beat = if matches!(pending.target, RouteTarget::Session(_)) && !is_audio {
                timing_view_for(self.revision.session(), self.periodic_tempo, self.anchor)
                    .frame_to_beat(performance_frame)
                    .map_err(|_| SessionPlaybackError::TempoConversion)?
            } else {
                pending.beat
            };
            self.route_scratch[index].owner =
                self.owner_for_target(pending.target, target_beat, performance_frame)?;
            self.route_scratch[index].pending = None;
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

    #[allow(clippy::too_many_arguments)]
    fn render_route_chunk<P: MediaPages + ?Sized>(
        &mut self,
        performance_frame: u64,
        arrangement_frame: u64,
        arrangement_loop_pass: u64,
        output: &mut [Frame],
        mut pages: Option<&mut P>,
        frame_offset: usize,
        observer: &mut Option<&mut crate::NodeSignalObserver<'_>>,
        capture: &mut Option<&mut OutputCapture<'_, '_>>,
    ) -> Result<(), SessionPlaybackError> {
        let revision = Arc::clone(&self.revision);
        let tracks = revision.arrangement().tracks();
        for scratch in &mut self.return_scratch {
            scratch[..output.len()].fill([0.0; 2]);
        }
        let timing = timing_view_for(self.revision.session(), self.periodic_tempo, self.anchor);
        let effect_clock = crate::effect_clock::ArrangementEffectClock::session(
            revision.session().timing(),
            arrangement_frame,
            performance_frame,
            arrangement_loop_pass,
            self.playing,
        );
        let table = self.revision.session().sine_table().as_ref();
        let mut release_cursor = 0;
        for index in 0..tracks.len() {
            let track = tracks
                .get(index)
                .ok_or(SessionPlaybackError::UnknownTrack)?;
            let capture_id = OutputCaptureId::Track(self.route_scratch[index].id);
            let recording_override = self.gain_recording_override.filter(|_| {
                self.revision.gain_recording_track() == Some(self.route_scratch[index].id)
            });
            let stateful_effects = self
                .device_chains
                .get(index)
                .is_some_and(|chain| !chain.is_empty());
            let mutable_active = self.mutable_instruments.get(index).is_some_and(|runtime| {
                runtime
                    .as_ref()
                    .is_some_and(|runtime| !runtime.committed_is_quiescent())
            });
            let metered_instrument = self.device_metering && track.instrument_id.is_some();
            let live_target = self
                .computer_keyboard
                .as_ref()
                .is_some_and(|keyboard| keyboard.track() == self.route_scratch[index].id);
            let buffered = live_target
                || stateful_effects
                || mutable_active
                || !track.sends().is_empty()
                || track.has_gain_automation()
                || metered_instrument;
            let ordered = live_target || track.requires_ordered_processing();
            let already_mixed = metered_instrument
                && !ordered
                && !stateful_effects
                && track.sends().is_empty()
                && !track.has_gain_automation();
            while self
                .release_scratch
                .get(release_cursor)
                .is_some_and(|entry| entry.location.track_index() < index)
            {
                release_cursor += 1;
            }
            let first_release = release_cursor;
            while self
                .release_scratch
                .get(release_cursor)
                .is_some_and(|entry| entry.location.track_index() == index)
            {
                release_cursor += 1;
            }
            let has_current_release = self.release_scratch[first_release..release_cursor]
                .iter()
                .any(|entry| entry.location.tail_group().is_none());
            let midi_route = effective_midi_route(&self.revision, &self.route_scratch, index)?;
            let has_current_route = live_target
                || !matches!(self.route_scratch[index].owner, RouteOwner::Silence)
                || !matches!(midi_route.owner, RouteOwner::Silence);
            let has_current_effect_tail = !has_current_route
                && !has_current_release
                && self
                    .device_chains
                    .get(index)
                    .is_some_and(|chain| !chain.committed_range_is_quiescent(0..chain.len()));
            if (buffered || ordered)
                && (has_current_route
                    || has_current_release
                    || has_current_effect_tail
                    || live_target)
            {
                let mut track_buffer = [[0.0; 2]; 256];
                let render_output = &mut track_buffer[..output.len()];
                if ordered {
                    render_route_source_chunk(
                        RouteSourceContext {
                            revision: &self.revision,
                            route: self.route_scratch[index],
                            historical_floor: self.historical_floor_for(index),
                            suppressions: &self.suppression_scratch,
                            continuations: &self.analytic_continuations,
                            index,
                            raw: live_target,
                            timing: &timing,
                            table,
                        },
                        &mut self.spans,
                        performance_frame,
                        arrangement_frame,
                        render_output,
                        pages.as_deref_mut(),
                        SourceStage::Audio,
                    )?;
                    if stateful_effects {
                        if let Some(observer) = observer.as_mut() {
                            let mut observe =
                                |owner: hex_lattice::ProcessorChainOwner,
                                 device_id: u64,
                                 device_type: hex_lattice_contract::DeviceTypeId,
                                 phase: crate::NodeSignalPhase,
                                 offset: usize,
                                 frames: &[Frame]| {
                                    observer(
                                        crate::PostNodeIdentity::new(owner, device_id, device_type),
                                        phase,
                                        offset,
                                        frames,
                                    );
                                };
                            self.device_chains[index]
                                .process_range_tapped_with_clock(
                                    0..track.instrument_position(),
                                    render_output,
                                    frame_offset,
                                    Some(&effect_clock),
                                    &mut observe,
                                )
                                .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
                        } else {
                            self.device_chains[index]
                                .process_range_with_clock(
                                    0..track.instrument_position(),
                                    render_output,
                                    Some(&effect_clock),
                                )
                                .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
                        }
                    } else {
                        track.apply_ordered_prefix(render_output);
                    }
                    track.apply_instrument_input_gain(render_output);
                    if self.device_metering {
                        track.instrument_meter.observe_input(render_output);
                    }
                    for release in &self.release_scratch[first_release..release_cursor] {
                        if release.location.tail_group().is_none() {
                            render_release_raw(
                                release,
                                table,
                                self.revision.arrangement().shared_contexts(),
                                &self.retained_contexts,
                                &self.analytic_continuations,
                                performance_frame,
                                render_output,
                            )?;
                        }
                    }
                    render_route_source_chunk(
                        RouteSourceContext {
                            revision: &self.revision,
                            route: midi_route,
                            historical_floor: self.historical_floor_for(index),
                            suppressions: &self.suppression_scratch,
                            continuations: &self.analytic_continuations,
                            index,
                            raw: live_target,
                            timing: &timing,
                            table,
                        },
                        &mut self.spans,
                        performance_frame,
                        arrangement_frame,
                        render_output,
                        pages.as_deref_mut(),
                        SourceStage::Instrument,
                    )?;
                    render_mutable_instrument_chunk(
                        MutableInstrumentChunkContext {
                            revision: &self.revision,
                            routes: &self.route_scratch,
                            runtimes: &mut self.mutable_instruments,
                            onsets: &mut self.mutable_onsets,
                            suppressions: &self.suppression_scratch,
                            events: &mut self.mutable_events,
                            spans: &mut self.spans,
                            performance_frame,
                            arrangement_frame,
                            arrangement_loop_pass,
                            loop_region: if self.linear_arrangement_export {
                                None
                            } else {
                                self.revision
                                    .arrangement()
                                    .loop_region()
                                    .filter(|region| region.enabled())
                            },
                            timing: &timing,
                        },
                        index,
                        render_output,
                    )?;
                    if let Some(keyboard) = self
                        .computer_keyboard
                        .as_mut()
                        .filter(|keyboard| keyboard.track() == self.route_scratch[index].id)
                    {
                        let clock = keyboard
                            .musical_clock(&timing, performance_frame, true)
                            .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                        keyboard
                            .render(render_output, Some(&clock))
                            .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                    }
                    track.apply_instrument_output_gain(render_output);
                    if self.device_metering {
                        track.instrument_meter.observe_output(render_output);
                    }
                    if stateful_effects {
                        let chain = self
                            .device_chains
                            .get_mut(index)
                            .ok_or(SessionPlaybackError::UnknownTrack)?;
                        let effect_count = chain.len();
                        if let Some(observer) = observer.as_mut() {
                            let mut observe =
                                |owner: hex_lattice::ProcessorChainOwner,
                                 device_id: u64,
                                 device_type: hex_lattice_contract::DeviceTypeId,
                                 phase: crate::NodeSignalPhase,
                                 offset: usize,
                                 frames: &[Frame]| {
                                    observer(
                                        crate::PostNodeIdentity::new(owner, device_id, device_type),
                                        phase,
                                        offset,
                                        frames,
                                    );
                                };
                            chain
                                .process_range_tapped_with_clock(
                                    track.instrument_position()..effect_count,
                                    render_output,
                                    frame_offset,
                                    Some(&effect_clock),
                                    &mut observe,
                                )
                                .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
                        } else {
                            chain
                                .process_range_with_clock(
                                    track.instrument_position()..effect_count,
                                    render_output,
                                    Some(&effect_clock),
                                )
                                .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
                        }
                    } else {
                        track.apply_ordered_suffix(render_output);
                    }
                } else {
                    for release in &self.release_scratch[first_release..release_cursor] {
                        if release.location.tail_group().is_none() {
                            if already_mixed {
                                render_release_captured(
                                    release,
                                    table,
                                    self.revision.arrangement().shared_contexts(),
                                    &self.retained_contexts,
                                    &self.analytic_continuations,
                                    performance_frame,
                                    render_output,
                                    None,
                                )?;
                                continue;
                            }
                            render_release_raw(
                                release,
                                table,
                                self.revision.arrangement().shared_contexts(),
                                &self.retained_contexts,
                                &self.analytic_continuations,
                                performance_frame,
                                render_output,
                            )?;
                        }
                    }
                    if metered_instrument {
                        let mut input = [[0.0; 2]; 256];
                        let input = &mut input[..output.len()];
                        render_route_source_chunk(
                            RouteSourceContext {
                                revision: &self.revision,
                                route: self.route_scratch[index],
                                historical_floor: self.historical_floor_for(index),
                                suppressions: &self.suppression_scratch,
                                continuations: &self.analytic_continuations,
                                index,
                                raw: false,
                                timing: &timing,
                                table,
                            },
                            &mut self.spans,
                            performance_frame,
                            arrangement_frame,
                            input,
                            pages.as_deref_mut(),
                            SourceStage::Audio,
                        )?;
                        track.instrument_meter.observe_mixed(
                            input,
                            track.source_mix().channel_gain(),
                            true,
                        );
                    }
                    render_route_source_chunk(
                        RouteSourceContext {
                            revision: &self.revision,
                            route: self.route_scratch[index],
                            historical_floor: self.historical_floor_for(index),
                            suppressions: &self.suppression_scratch,
                            continuations: &self.analytic_continuations,
                            index,
                            raw: false,
                            timing: &timing,
                            table,
                        },
                        &mut self.spans,
                        performance_frame,
                        arrangement_frame,
                        render_output,
                        pages.as_deref_mut(),
                        SourceStage::Whole,
                    )?;
                    if metered_instrument {
                        track.instrument_meter.observe_mixed(
                            render_output,
                            track.source_mix().channel_gain(),
                            false,
                        );
                    }
                    let chain = self
                        .device_chains
                        .get_mut(index)
                        .ok_or(SessionPlaybackError::UnknownTrack)?;
                    if let Some(observer) = observer.as_mut() {
                        let mut observe =
                            |owner: hex_lattice::ProcessorChainOwner,
                             device_id: u64,
                             device_type: hex_lattice_contract::DeviceTypeId,
                             phase: crate::NodeSignalPhase,
                             offset: usize,
                             frames: &[Frame]| {
                                observer(
                                    crate::PostNodeIdentity::new(owner, device_id, device_type),
                                    phase,
                                    offset,
                                    frames,
                                );
                            };
                        chain
                            .process_range_tapped_with_clock(
                                0..chain.len(),
                                render_output,
                                frame_offset,
                                Some(&effect_clock),
                                &mut observe,
                            )
                            .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
                    } else {
                        chain
                            .process_range_with_clock(
                                0..chain.len(),
                                render_output,
                                Some(&effect_clock),
                            )
                            .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
                    }
                }
                for (frame_index, source) in render_output.iter_mut().enumerate() {
                    let mix = if let Some(gain) = recording_override {
                        track.recording_mix(gain)
                    } else if track.has_gain_automation() {
                        track.mix_at(
                            arrangement_frame
                                .checked_add(frame_index as u64)
                                .ok_or(SessionPlaybackError::ArithmeticOverflow)?,
                        )
                    } else {
                        track.mix()
                    };
                    let mix = if live_target {
                        mix.without_effects()
                    } else {
                        mix
                    };
                    if already_mixed {
                        // Preserve the analytic per-voice fader summation order.
                        output[frame_index][0] += source[0];
                        output[frame_index][1] += source[1];
                    } else if track.sends().is_empty() {
                        mix.add_frame(&mut output[frame_index], *source);
                    } else {
                        let contribution = mix.apply_frame(*source);
                        output[frame_index][0] += contribution[0];
                        output[frame_index][1] += contribution[1];
                        for send in track.sends() {
                            let target = self
                                .return_scratch
                                .get_mut(send.return_index)
                                .ok_or(SessionPlaybackError::DeviceRuntime)?;
                            target[frame_index][0] += contribution[0] * send.gain;
                            target[frame_index][1] += contribution[1] * send.gain;
                        }
                    }
                    if capture.is_some() && !already_mixed {
                        *source = mix.apply_frame(*source);
                    }
                }
                if let Some(capture) = capture.as_mut() {
                    capture.add(capture_id, frame_offset, render_output)?;
                }
            } else {
                let selected = capture
                    .as_ref()
                    .is_some_and(|capture| capture.selects(capture_id));
                if selected {
                    let mut captured = [[0.0; 2]; 256];
                    for release in &self.release_scratch[first_release..release_cursor] {
                        if release.location.tail_group().is_none() {
                            render_release_captured(
                                release,
                                table,
                                self.revision.arrangement().shared_contexts(),
                                &self.retained_contexts,
                                &self.analytic_continuations,
                                performance_frame,
                                output,
                                Some(&mut captured[..output.len()]),
                            )?;
                        }
                    }

                    track
                        .render_additive_captured(
                            arrangement_frame,
                            performance_frame,
                            output,
                            &self.suppression_scratch,
                            pages.as_deref_mut(),
                            &mut captured[..output.len()],
                            &self.analytic_continuations,
                        )
                        .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                    if let Some(capture) = capture.as_mut() {
                        capture.add(capture_id, frame_offset, &captured[..output.len()])?;
                    }
                    continue;
                }
                for release in &self.release_scratch[first_release..release_cursor] {
                    if release.location.tail_group().is_none() {
                        render_release_captured(
                            release,
                            table,
                            self.revision.arrangement().shared_contexts(),
                            &self.retained_contexts,
                            &self.analytic_continuations,
                            performance_frame,
                            output,
                            None,
                        )?;
                    }
                }
                render_route_source_chunk(
                    RouteSourceContext {
                        revision: &self.revision,
                        route: self.route_scratch[index],
                        historical_floor: self.historical_floor_for(index),
                        suppressions: &self.suppression_scratch,
                        continuations: &self.analytic_continuations,
                        index,
                        raw: false,
                        timing: &timing,
                        table,
                    },
                    &mut self.spans,
                    performance_frame,
                    arrangement_frame,
                    output,
                    pages.as_deref_mut(),
                    SourceStage::Whole,
                )?;
            }
        }
        self.render_detached_tails(performance_frame, output, frame_offset, capture)?;
        self.render_return_buses(output, frame_offset, Some(&effect_clock), observer, capture)
    }

    fn render_return_buses(
        &mut self,
        output: &mut [Frame],
        frame_offset: usize,
        effect_clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
        observer: &mut Option<&mut crate::NodeSignalObserver<'_>>,
        capture: &mut Option<&mut OutputCapture<'_, '_>>,
    ) -> Result<(), SessionPlaybackError> {
        let arrangement = self.revision.arrangement();
        if !arrangement.return_feed_plan.is_empty() {
            for &index in arrangement.return_feed_plan.order() {
                let bus = &arrangement.return_buses()[index];
                let frames = &mut self.return_scratch[index][..output.len()];
                let chain = self
                    .return_chains
                    .get_mut(index)
                    .ok_or(SessionPlaybackError::DeviceRuntime)?;
                if let Some(observer) = observer.as_mut() {
                    let mut observe =
                        |owner, device_id, device_type, phase, offset, frames: &[Frame]| {
                            observer(
                                crate::PostNodeIdentity::new(owner, device_id, device_type),
                                phase,
                                offset,
                                frames,
                            );
                        };
                    chain
                        .process_range_tapped_with_clock(
                            0..chain.len(),
                            frames,
                            frame_offset,
                            effect_clock,
                            &mut observe,
                        )
                        .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
                } else {
                    chain
                        .process_range_with_clock(0..chain.len(), frames, effect_clock)
                        .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
                }
                if let Some(matrix) = bus.matrix {
                    matrix.apply_block(frames);
                }
                if bus.muted {
                    frames.fill([0.0; 2]);
                } else {
                    for frame in frames {
                        frame[0] *= bus.gain;
                        frame[1] *= bus.gain;
                    }
                }
                if let Some(capture) = capture.as_mut() {
                    capture.add(
                        OutputCaptureId::Return(bus.id),
                        frame_offset,
                        &self.return_scratch[index][..output.len()],
                    )?;
                }
                arrangement.return_feed_plan.propagate(
                    index,
                    &mut self.return_scratch,
                    output.len(),
                );
            }
            let count = output.len();
            for frames in &self.return_scratch {
                for (destination, source) in output.iter_mut().zip(&frames[..count]) {
                    destination[0] += source[0];
                    destination[1] += source[1];
                }
            }
            // Captured chains feed Main only, never any current or old edge.
            if self.playing {
                self.return_effect_tails
                    .render_chunk(output, frame_offset, capture)?;
            }
            return Ok(());
        }
        for (index, bus) in self
            .revision
            .arrangement()
            .return_buses()
            .iter()
            .enumerate()
        {
            let frames = self
                .return_scratch
                .get_mut(index)
                .ok_or(SessionPlaybackError::DeviceRuntime)?;
            let frames = &mut frames[..output.len()];
            let chain = self
                .return_chains
                .get_mut(index)
                .ok_or(SessionPlaybackError::DeviceRuntime)?;
            if let Some(observer) = observer.as_mut() {
                let mut observe = |owner: hex_lattice::ProcessorChainOwner,
                                   device_id: u64,
                                   device_type: hex_lattice_contract::DeviceTypeId,
                                   phase: crate::NodeSignalPhase,
                                   offset: usize,
                                   frames: &[Frame]| {
                    observer(
                        crate::PostNodeIdentity::new(owner, device_id, device_type),
                        phase,
                        offset,
                        frames,
                    );
                };
                chain
                    .process_range_tapped_with_clock(
                        0..chain.len(),
                        frames,
                        frame_offset,
                        effect_clock,
                        &mut observe,
                    )
                    .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
            } else {
                chain
                    .process_range_with_clock(0..chain.len(), frames, effect_clock)
                    .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
            }
            if let Some(matrix) = bus.matrix {
                matrix.apply_block(frames);
            }
            if !bus.muted {
                for (destination, source) in output.iter_mut().zip(frames.iter()) {
                    destination[0] += source[0] * bus.gain;
                    destination[1] += source[1] * bus.gain;
                }
            }
            if let Some(capture) = capture.as_mut() {
                for frame in frames.iter_mut() {
                    *frame = if bus.muted {
                        [0.0; 2]
                    } else {
                        [frame[0] * bus.gain, frame[1] * bus.gain]
                    };
                }
                capture.add(OutputCaptureId::Return(bus.id), frame_offset, frames)?;
            }
        }
        if self.playing {
            self.return_effect_tails
                .render_chunk(output, frame_offset, capture)?;
        }
        Ok(())
    }
    fn render_detached_tails(
        &mut self,
        performance_frame: u64,
        output: &mut [Frame],
        frame_offset: usize,
        capture: &mut Option<&mut OutputCapture<'_, '_>>,
    ) -> Result<(), SessionPlaybackError> {
        let timing = timing_view_for(self.revision.session(), self.periodic_tempo, self.anchor);
        let table = self.revision.session().sine_table().as_ref();
        // Removed stateless tracks retain their copied linear mix. Stateful
        // tails are grouped by their captured old runtime chain and old mix.
        let mut rendered_mutable_tails = [false; SESSION_TAIL_GROUP_CAPACITY];
        let mut rendered_tail_suffixes = [false; SESSION_TAIL_GROUP_CAPACITY];
        let mut tail_cursor = self.release_scratch.partition_point(|entry| {
            entry.location.track_index() < self.revision.arrangement().tracks().len()
        });
        while tail_cursor < self.release_scratch.len() {
            let release = self
                .release_scratch
                .get(tail_cursor)
                .ok_or(SessionPlaybackError::ReleaseBankFull)?;
            let Some(group) = release.location.tail_group() else {
                let mut captured = [[0.0; 2]; 256];
                render_release_captured(
                    release,
                    table,
                    self.revision.arrangement().shared_contexts(),
                    &self.retained_contexts,
                    &self.analytic_continuations,
                    performance_frame,
                    output,
                    capture
                        .as_ref()
                        .is_some_and(|capture| {
                            capture.selects(OutputCaptureId::Track(release.track_id))
                        })
                        .then_some(&mut captured[..output.len()]),
                )?;
                if let Some(capture) = capture.as_mut() {
                    capture.add(
                        OutputCaptureId::Track(release.track_id),
                        frame_offset,
                        &captured[..output.len()],
                    )?;
                }
                tail_cursor += 1;
                continue;
            };
            let group_start = tail_cursor;
            while self
                .release_scratch
                .get(tail_cursor)
                .is_some_and(|entry| entry.location.tail_group() == Some(group))
            {
                tail_cursor += 1;
            }
            let group_index = usize::from(group);
            let chain = self
                .tail_groups
                .get_mut(group_index)
                .and_then(Option::as_mut)
                .ok_or(SessionPlaybackError::DeviceRuntime)?;
            let mut tail_buffer = [[0.0; 2]; 256];
            let tail_output = &mut tail_buffer[..output.len()];
            for release in &self.release_scratch[group_start..tail_cursor] {
                render_release_raw(
                    release,
                    table,
                    self.revision.arrangement().shared_contexts(),
                    &self.retained_contexts,
                    &self.analytic_continuations,
                    performance_frame,
                    tail_output,
                )?;
            }
            if let Some(tail) = self
                .tail_mutable_instruments
                .get_mut(group_index)
                .and_then(Option::as_mut)
            {
                let rendered = rendered_mutable_tails
                    .get_mut(group_index)
                    .ok_or(SessionPlaybackError::DeviceRuntime)?;
                *rendered = true;
                if !tail.runtime.speculative_is_quiescent() {
                    process_mutable_tail(
                        tail,
                        &mut self.mutable_onsets,
                        &mut self.mutable_events,
                        performance_frame,
                        tail_output,
                        &timing,
                    )?;
                }
            }
            if !chain.is_empty() {
                let suffix_start = self
                    .tail_suffix_starts
                    .get(group_index)
                    .copied()
                    .ok_or(SessionPlaybackError::DeviceRuntime)?;
                let effect_count = chain.len();
                chain
                    .process_range(suffix_start..effect_count, tail_output)
                    .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
                let active = !chain.speculative_range_is_quiescent(suffix_start..effect_count);
                *self
                    .tail_output_scratch
                    .get_mut(group_index)
                    .ok_or(SessionPlaybackError::DeviceRuntime)? = active;
            }
            *rendered_tail_suffixes
                .get_mut(group_index)
                .ok_or(SessionPlaybackError::DeviceRuntime)? = true;
            if let Some(matrix) = self
                .tail_suffix_matrices
                .get(group_index)
                .copied()
                .flatten()
            {
                matrix.apply_block(tail_output);
            }
            let mix = self
                .tail_mixes
                .get(group_index)
                .copied()
                .ok_or(SessionPlaybackError::DeviceRuntime)?;
            for (destination, source) in output.iter_mut().zip(tail_output.iter().copied()) {
                mix.add_frame(destination, source);
            }
            if let Some(capture) = capture.as_mut() {
                let owner =
                    self.tail_track_ids[group_index].ok_or(SessionPlaybackError::DeviceRuntime)?;
                for frame in tail_output.iter_mut() {
                    *frame = mix.apply_frame(*frame);
                }
                capture.add(OutputCaptureId::Track(owner), frame_offset, tail_output)?;
            }
        }
        for group_index in 0..self.tail_mutable_instruments.len() {
            if rendered_mutable_tails
                .get(group_index)
                .copied()
                .unwrap_or(true)
            {
                continue;
            }
            let Some(tail) = self
                .tail_mutable_instruments
                .get_mut(group_index)
                .and_then(Option::as_mut)
            else {
                continue;
            };
            if tail.runtime.speculative_is_quiescent() {
                continue;
            }
            let mut tail_buffer = [[0.0; 2]; 256];
            let tail_output = &mut tail_buffer[..output.len()];
            process_mutable_tail(
                tail,
                &mut self.mutable_onsets,
                &mut self.mutable_events,
                performance_frame,
                tail_output,
                &timing,
            )?;
            let chain = self
                .tail_groups
                .get_mut(group_index)
                .and_then(Option::as_mut)
                .ok_or(SessionPlaybackError::DeviceRuntime)?;
            if !chain.is_empty() {
                let suffix_start = self
                    .tail_suffix_starts
                    .get(group_index)
                    .copied()
                    .ok_or(SessionPlaybackError::DeviceRuntime)?;
                let effect_count = chain.len();
                chain
                    .process_range(suffix_start..effect_count, tail_output)
                    .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
                let active = !chain.speculative_range_is_quiescent(suffix_start..effect_count);
                *self
                    .tail_output_scratch
                    .get_mut(group_index)
                    .ok_or(SessionPlaybackError::DeviceRuntime)? = active;
                *rendered_tail_suffixes
                    .get_mut(group_index)
                    .ok_or(SessionPlaybackError::DeviceRuntime)? = true;
            }
            if let Some(matrix) = self
                .tail_suffix_matrices
                .get(group_index)
                .copied()
                .flatten()
            {
                matrix.apply_block(tail_output);
            }
            let mix = self
                .tail_mixes
                .get(group_index)
                .copied()
                .ok_or(SessionPlaybackError::DeviceRuntime)?;
            for (destination, source) in output.iter_mut().zip(tail_output.iter().copied()) {
                mix.add_frame(destination, source);
            }
            if let Some(capture) = capture.as_mut() {
                let owner =
                    self.tail_track_ids[group_index].ok_or(SessionPlaybackError::DeviceRuntime)?;
                for frame in tail_output.iter_mut() {
                    *frame = mix.apply_frame(*frame);
                }
                capture.add(OutputCaptureId::Track(owner), frame_offset, tail_output)?;
            }
        }
        // Once an old source has stopped, its committed effect history still
        // owns output. Advance that suffix exactly once per render chunk using
        // real zero input, then keep its captured matrix and mix in the path.
        for group_index in 0..self.tail_groups.len() {
            if rendered_tail_suffixes
                .get(group_index)
                .copied()
                .unwrap_or(true)
                || !self
                    .tail_output_scratch
                    .get(group_index)
                    .copied()
                    .unwrap_or(false)
            {
                continue;
            }
            let chain = self
                .tail_groups
                .get_mut(group_index)
                .and_then(Option::as_mut)
                .ok_or(SessionPlaybackError::DeviceRuntime)?;
            let suffix_start = self
                .tail_suffix_starts
                .get(group_index)
                .copied()
                .ok_or(SessionPlaybackError::DeviceRuntime)?;
            let effect_count = chain.len();
            if suffix_start > effect_count {
                return Err(SessionPlaybackError::DeviceRuntime);
            }
            let mut tail_buffer = [[0.0; 2]; 256];
            let tail_output = &mut tail_buffer[..output.len()];
            chain
                .process_range(suffix_start..effect_count, tail_output)
                .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
            let active = !chain.speculative_range_is_quiescent(suffix_start..effect_count);
            *self
                .tail_output_scratch
                .get_mut(group_index)
                .ok_or(SessionPlaybackError::DeviceRuntime)? = active;
            if let Some(matrix) = self
                .tail_suffix_matrices
                .get(group_index)
                .copied()
                .flatten()
            {
                matrix.apply_block(tail_output);
            }
            let mix = self
                .tail_mixes
                .get(group_index)
                .copied()
                .ok_or(SessionPlaybackError::DeviceRuntime)?;
            for (destination, source) in output.iter_mut().zip(tail_output.iter().copied()) {
                mix.add_frame(destination, source);
            }
            if let Some(capture) = capture.as_mut() {
                let owner =
                    self.tail_track_ids[group_index].ok_or(SessionPlaybackError::DeviceRuntime)?;
                for frame in tail_output.iter_mut() {
                    *frame = mix.apply_frame(*frame);
                }
                capture.add(OutputCaptureId::Track(owner), frame_offset, tail_output)?;
            }
        }
        Ok(())
    }
}

fn process_mutable_tail<T: MidiTiming + ?Sized>(
    tail: &mut super::MutableInstrumentTail,
    onsets: &mut MutableOnsetBuffer,
    events: &mut Vec<hex_lattice::NativeInstrumentEvent>,
    performance_frame: u64,
    output: &mut [Frame],
    timing: &T,
) -> Result<(), SessionPlaybackError> {
    onsets.clear();
    tail.runtime
        .process_segment_with_clock(
            tail.track_id,
            performance_frame,
            performance_frame,
            output,
            onsets,
            &[],
            events,
            Some(&InstrumentClock {
                timing,
                start: performance_frame,
            }),
        )
        .map_err(crate::mutable_instrument::MutableInstrumentRuntimeError::render_error)
}

struct MutableInstrumentChunkContext<'a, T: ?Sized> {
    revision: &'a PreparedRevision,
    routes: &'a [super::TrackRoute],
    runtimes: &'a mut [Option<MutableInstrumentTrackRuntime>],
    onsets: &'a mut MutableOnsetBuffer,
    suppressions: &'a [MidiOccurrenceSuppression],
    events: &'a mut Vec<hex_lattice::NativeInstrumentEvent>,
    spans: &'a mut MidiSpanBuffer,
    performance_frame: u64,
    arrangement_frame: u64,
    arrangement_loop_pass: u64,
    loop_region: Option<hex_project::LoopRegion>,
    timing: &'a T,
}

fn render_mutable_instrument_chunk<T: MidiTiming + ?Sized>(
    context: MutableInstrumentChunkContext<'_, T>,
    index: usize,
    output: &mut [Frame],
) -> Result<(), SessionPlaybackError> {
    let MutableInstrumentChunkContext {
        revision,
        routes,
        runtimes,
        onsets,
        suppressions,
        events,
        spans,
        performance_frame,
        arrangement_frame,
        arrangement_loop_pass,
        loop_region,
        timing,
    } = context;
    if runtimes.get(index).and_then(Option::as_ref).is_none() {
        return Ok(());
    }
    let track = revision
        .arrangement()
        .tracks()
        .get(index)
        .ok_or(SessionPlaybackError::UnknownTrack)?;
    let track_id = track
        .canonical_track_id()
        .ok_or(SessionPlaybackError::RevisionTrackMismatch)?;
    let source_end = performance_frame
        .checked_add(
            u64::try_from(output.len()).map_err(|_| SessionPlaybackError::ArithmeticOverflow)?,
        )
        .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
    onsets.clear();
    let midi_route = effective_midi_route(revision, routes, index)?;
    match midi_route.owner {
        RouteOwner::Arrangement => {
            let mut event_error = false;
            track
                .for_each_mutable_midi_span(
                    arrangement_frame,
                    arrangement_frame
                        .checked_add(
                            u64::try_from(output.len())
                                .map_err(|_| SessionPlaybackError::ArithmeticOverflow)?,
                        )
                        .ok_or(SessionPlaybackError::ArithmeticOverflow)?,
                    |span| {
                        let is_wrap_chase = arrangement_loop_pass > 0
                            && loop_region.is_some_and(|region| {
                                arrangement_frame == region.start()
                                    && span.start < region.start()
                                    && span.gate_end > region.start()
                            });
                        let source_start = if is_wrap_chase {
                            arrangement_frame
                        } else {
                            span.start
                        };
                        let source_gate_end = if is_wrap_chase {
                            loop_region
                                .map(|region| span.gate_end.min(region.end()))
                                .ok_or(())
                        } else {
                            Ok(loop_region
                                .map_or(span.gate_end, |region| span.gate_end.min(region.end())))
                        };
                        let Ok(source_gate_end) = source_gate_end else {
                            event_error = true;
                            return;
                        };
                        event_error |= onsets
                            .push(crate::mutable_instrument::MutableNoteOnset {
                                key: crate::mutable_instrument::MutableOccurrenceKey::new(
                                    track_id,
                                    span.clip,
                                    span.note,
                                    span.iteration,
                                    arrangement_loop_pass,
                                ),
                                onset_beat: span.onset_beat,
                                gate_end_beat: span.gate_end_beat,
                                source_start,
                                source_gate_end,
                                pitch: span.pitch,
                                channel: span.channel,
                                velocity: span.velocity,
                            })
                            .is_err();
                    },
                )
                .map_err(|_| SessionPlaybackError::MutableInstrumentEvents)?;
            if event_error {
                return Err(SessionPlaybackError::MutableInstrumentEvents);
            }
        }
        RouteOwner::Session { clip, launch } => {
            let prepared = revision
                .session()
                .midi_clip_for_track(clip, track_id)
                .ok_or(SessionPlaybackError::UnknownClip)?;
            if prepared.track != routes[index].id {
                return Err(SessionPlaybackError::ClipTrackMismatch);
            }
            if prepared.instrument_enabled() {
                prepared
                    .instrument_template()
                    .query(launch, timing, performance_frame, source_end, spans)
                    .map_err(SessionPlaybackError::Midi)?;
                for span in spans.spans() {
                    onsets
                        .push(crate::mutable_instrument::MutableNoteOnset {
                            key: crate::mutable_instrument::MutableOccurrenceKey::new(
                                prepared.track,
                                span.clip,
                                span.note,
                                span.iteration,
                                0,
                            ),
                            onset_beat: span.onset_beat,
                            gate_end_beat: span.gate_end_beat,
                            source_start: span.start_frame,
                            source_gate_end: span.gate_end_frame,
                            pitch: span.pitch,
                            channel: span.channel,
                            velocity: span.velocity,
                        })
                        .map_err(|_| SessionPlaybackError::MutableInstrumentEvents)?;
                }
            }
        }
        RouteOwner::SessionAudio { .. } | RouteOwner::Silence => {}
    }
    let runtime = runtimes
        .get_mut(index)
        .and_then(Option::as_mut)
        .ok_or(SessionPlaybackError::MutableInstrumentRuntime)?;
    runtime
        .process_segment_with_clock(
            routes[index].id,
            performance_frame,
            match midi_route.owner {
                RouteOwner::Arrangement => arrangement_frame,
                _ => performance_frame,
            },
            output,
            onsets,
            suppressions,
            events,
            Some(&InstrumentClock {
                timing,
                start: performance_frame,
            }),
        )
        .map_err(crate::mutable_instrument::MutableInstrumentRuntimeError::render_error)
}

struct RouteSourceContext<'a, T> {
    revision: &'a PreparedRevision,
    route: super::TrackRoute,
    historical_floor: Option<BeatTime>,
    suppressions: &'a [MidiOccurrenceSuppression],
    continuations: &'a [Option<hex_lattice::AnalyticContinuation>],
    index: usize,
    raw: bool,
    timing: &'a T,
    table: &'a hex_devices::SineTable,
}

#[derive(Clone, Copy)]
enum SourceStage {
    Whole,
    Audio,
    Instrument,
}

fn render_route_source_chunk<P: MediaPages + ?Sized, T: MidiTiming>(
    context: RouteSourceContext<'_, T>,
    spans: &mut MidiSpanBuffer,
    performance_frame: u64,
    arrangement_frame: u64,
    output: &mut [Frame],
    mut pages: Option<&mut P>,
    stage: SourceStage,
) -> Result<(), SessionPlaybackError> {
    let RouteSourceContext {
        revision,
        route,
        historical_floor,
        suppressions,
        continuations,
        index,
        raw,
        timing,
        table,
    } = context;
    let track = revision
        .arrangement()
        .tracks()
        .get(index)
        .ok_or(SessionPlaybackError::UnknownTrack)?;
    let source_mix = if raw {
        crate::PreparedTrackMix::identity()
    } else {
        track.source_mix()
    };
    match route.owner {
        RouteOwner::Arrangement => {
            if raw && matches!(stage, SourceStage::Audio) {
                return track
                    .render_raw_audio(arrangement_frame, output, pages.as_deref_mut())
                    .map_err(|error| SessionPlaybackError::Arrangement(error.0));
            }
            if raw && matches!(stage, SourceStage::Instrument) {
                return track
                    .render_raw_instrument(
                        arrangement_frame,
                        performance_frame,
                        output,
                        suppressions,
                        continuations,
                    )
                    .map_err(|error| SessionPlaybackError::Arrangement(error.0));
            }
            let has_active_suppression = suppressions
                .iter()
                .any(|entry| entry.track == route.id && entry.until_frame > performance_frame);
            match stage {
                SourceStage::Whole => {
                    if has_active_suppression || continuations.iter().any(Option::is_some) {
                        track.render_additive_with_suppressions_and_pages(
                            arrangement_frame,
                            performance_frame,
                            output,
                            suppressions,
                            pages.as_deref_mut(),
                            continuations,
                        )
                    } else if let Some(page_provider) = pages.as_deref_mut() {
                        track.render_additive_with_pages(arrangement_frame, output, page_provider)
                    } else if track.has_gain_automation() {
                        // This is the raw source stage; the buffered caller applies gain once.
                        track.render_additive_with_midi_floor(arrangement_frame, output, None)
                    } else {
                        track.render_additive(arrangement_frame, output)
                    }
                }
                SourceStage::Audio => {
                    if let Some(page_provider) = pages.as_deref_mut() {
                        track.render_audio_additive_with_pages(
                            arrangement_frame,
                            output,
                            page_provider,
                        )
                    } else {
                        track.render_audio_additive(arrangement_frame, output)
                    }
                }
                SourceStage::Instrument => track.render_instrument_additive_with_suppressions(
                    arrangement_frame,
                    performance_frame,
                    output,
                    suppressions,
                    continuations,
                ),
            }
            .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
        }
        RouteOwner::Silence => {}
        RouteOwner::Session { clip, launch } => {
            if matches!(stage, SourceStage::Audio) {
                return Ok(());
            }
            let prepared = revision
                .session()
                .midi_clip_for_track(clip, route.id)
                .ok_or(SessionPlaybackError::UnknownClip)?;
            if prepared.track != route.id {
                return Err(SessionPlaybackError::ClipTrackMismatch);
            }
            if prepared.instrument_enabled() && !prepared.instrument_template().is_event_source() {
                let context = crate::MidiInstrumentRenderContext::new(launch, timing, table);
                let context = historical_floor.map_or(context, |floor| context.after_onset(floor));
                let context = context
                    .with_suppressions(suppressions)
                    .with_continuations(continuations);
                prepared
                    .instrument_template()
                    .render_additive(context, source_mix, performance_frame, output, spans)
                    .map_err(SessionPlaybackError::Midi)?;
            }
        }
        RouteOwner::SessionAudio { clip, launch_frame } => {
            if matches!(stage, SourceStage::Instrument) {
                return Ok(());
            }
            let prepared = revision
                .session()
                .audio_clip(clip)
                .ok_or(SessionPlaybackError::UnknownClip)?;
            if prepared.track != route.id {
                return Err(SessionPlaybackError::ClipTrackMismatch);
            }
            match prepared.source() {
                PreparedAudioSource::Resident(audio) => {
                    let frames = audio.frames();
                    for (offset, frame) in output.iter_mut().enumerate() {
                        let offset = u64::try_from(offset)
                            .map_err(|_| SessionPlaybackError::ArithmeticOverflow)?;
                        let absolute = performance_frame
                            .checked_add(offset)
                            .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
                        let Some(elapsed) = absolute.checked_sub(launch_frame) else {
                            continue;
                        };
                        let Some(source_frame) = prepared.source_start.checked_add(elapsed) else {
                            continue;
                        };
                        if source_frame >= prepared.source_end {
                            continue;
                        }
                        let source_index = usize::try_from(source_frame)
                            .map_err(|_| SessionPlaybackError::ArithmeticOverflow)?;
                        let mut sample = *frames
                            .get(source_index)
                            .ok_or(SessionPlaybackError::UnknownClip)?;
                        if prepared.gain != 1.0 {
                            sample[0] *= prepared.gain;
                            sample[1] *= prepared.gain;
                        }
                        source_mix.add_frame(frame, sample);
                    }
                }
                PreparedAudioSource::Streamed(source) => {
                    let page_provider = pages.ok_or(SessionPlaybackError::MediaPagesUnavailable)?;
                    let Some(mut elapsed) = performance_frame.checked_sub(launch_frame) else {
                        return Ok(());
                    };
                    let Some(mut source_frame) = prepared.source_start.checked_add(elapsed) else {
                        return Ok(());
                    };
                    let mut output_offset = 0_usize;
                    while output_offset < output.len() && source_frame < prepared.source_end {
                        let remaining =
                            usize::try_from(prepared.source_end.saturating_sub(source_frame))
                                .unwrap_or(usize::MAX);
                        let count = (output.len() - output_offset).min(remaining);
                        let offset = u64::try_from(output_offset)
                            .map_err(|_| SessionPlaybackError::ArithmeticOverflow)?;
                        let timeline_frame = performance_frame
                            .checked_add(offset)
                            .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
                        let rendered = render_streamed_page_span(
                            source_mix,
                            source,
                            StreamedPageSpan {
                                source_frame,
                                timeline_frame,
                                output_offset,
                                count,
                                gain: prepared.gain,
                                fade: None,
                            },
                            output,
                            page_provider,
                        );
                        if rendered == 0 {
                            break;
                        }
                        let rendered = u64::try_from(rendered)
                            .map_err(|_| SessionPlaybackError::ArithmeticOverflow)?;
                        elapsed = elapsed
                            .checked_add(rendered)
                            .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
                        source_frame = prepared
                            .source_start
                            .checked_add(elapsed)
                            .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
                        output_offset = output_offset
                            .checked_add(
                                usize::try_from(rendered)
                                    .map_err(|_| SessionPlaybackError::ArithmeticOverflow)?,
                            )
                            .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
                    }
                }
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn render_release_captured(
    release: &super::ReleaseEntry,
    _table: &hex_devices::SineTable,
    contexts: &crate::shared_analytic::SharedAnalyticContextTable,
    retained: &[Option<Arc<crate::shared_analytic::SharedContextOwner>>],
    continuations: &[Option<hex_lattice::AnalyticContinuation>],
    performance_frame: u64,
    output: &mut [Frame],
    mut capture: Option<&mut [Frame]>,
) -> Result<(), SessionPlaybackError> {
    for (offset, frame) in output.iter_mut().enumerate() {
        let relative =
            u64::try_from(offset).map_err(|_| SessionPlaybackError::ArithmeticOverflow)?;
        let absolute = performance_frame
            .checked_add(relative)
            .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
        if let Some(sample) = release
            .voice
            .sample_at_with_continuation(absolute, contexts, retained, continuations)
            .map_err(|_| SessionPlaybackError::SharedAnalyticContext)?
        {
            let contribution = release.mix.apply_frame(sample);
            frame[0] += contribution[0];
            frame[1] += contribution[1];
            if let Some(captured) = capture
                .as_deref_mut()
                .and_then(|frames| frames.get_mut(offset))
            {
                captured[0] += contribution[0];
                captured[1] += contribution[1];
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn render_release_raw(
    release: &super::ReleaseEntry,
    _table: &hex_devices::SineTable,
    contexts: &crate::shared_analytic::SharedAnalyticContextTable,
    retained: &[Option<Arc<crate::shared_analytic::SharedContextOwner>>],
    continuations: &[Option<hex_lattice::AnalyticContinuation>],
    performance_frame: u64,
    output: &mut [Frame],
) -> Result<(), SessionPlaybackError> {
    for (offset, frame) in output.iter_mut().enumerate() {
        let relative =
            u64::try_from(offset).map_err(|_| SessionPlaybackError::ArithmeticOverflow)?;
        let absolute = performance_frame
            .checked_add(relative)
            .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
        if let Some(sample) = release
            .voice
            .sample_at_with_continuation(absolute, contexts, retained, continuations)
            .map_err(|_| SessionPlaybackError::SharedAnalyticContext)?
        {
            frame[0] += sample[0];
            frame[1] += sample[1];
        }
    }
    Ok(())
}

fn next_pending_frame(routes: &[TrackRoute]) -> Option<u64> {
    routes
        .iter()
        .filter_map(|route| route.pending.map(|pending| pending.frame))
        .min()
}

pub(super) fn normalize_loop_cursor(
    frame: u64,
    region: LoopRegion,
    loop_pass: u64,
) -> Result<(u64, u64), SessionPlaybackError> {
    if frame < region.end() {
        return Ok((frame, loop_pass));
    }
    let length = region
        .end()
        .checked_sub(region.start())
        .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
    if length == 0 {
        return Err(SessionPlaybackError::ArithmeticOverflow);
    }
    let wraps = (frame - region.start()) / length;
    let loop_pass = loop_pass
        .checked_add(wraps)
        .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
    Ok((
        region.start() + (frame - region.start()) % length,
        loop_pass,
    ))
}

/// One borrowed effective performance timeline shared by every voice in a chunk.
struct InstrumentClock<'a, T: ?Sized> {
    timing: &'a T,
    start: u64,
}
impl<T: MidiTiming + ?Sized> hex_lattice::NativeInstrumentClock for InstrumentClock<'_, T> {
    fn performance_frame_at_offset(&self, offset: usize) -> Option<u64> {
        self.start.checked_add(u64::try_from(offset).ok()?)
    }
    fn transport_is_running(&self) -> Option<bool> {
        Some(true)
    }
    fn beat_at_offset(&self, offset: usize) -> Option<u64> {
        let frame = self.start.checked_add(u64::try_from(offset).ok()?)?;
        self.timing.frame_to_beat(frame).ok().map(|beat| beat.raw())
    }
}

#[cfg(test)]
mod observed_page_tests {
    use super::*;
    #[test]
    fn instrument_clock_exposes_nonzero_performance_across_content_loop_positions() {
        use hex_lattice::NativeInstrumentClock;
        let project = hex_project::Project::new(48_000).unwrap();
        let timing = crate::PreparedTempoMap::prepare(project.tempo_map().clone(), 48_000).unwrap();
        let clock = InstrumentClock {
            timing: &timing,
            start: 96_000,
        };
        assert_eq!(clock.performance_frame_at_offset(256), Some(96_256));
        assert_eq!(clock.transport_is_running(), Some(true));
        assert_eq!(clock.arrangement_frame_at_offset(0), None);
        assert_eq!(
            clock.beat_at_offset(256),
            timing.frame_to_beat(96_256).ok().map(|b| b.raw())
        );
        let exhausted = InstrumentClock {
            timing: &timing,
            start: u64::MAX,
        };
        assert_eq!(exhausted.performance_frame_at_offset(1), None);
    }
    struct NoPages {
        accesses: usize,
    }
    impl MediaPages for NoPages {
        fn page(
            &mut self,
            _: crate::StreamSourceKey,
            _: u64,
        ) -> Option<crate::BorrowedMediaPage<'_>> {
            self.accesses += 1;
            None
        }
        fn request_page(&mut self, _: crate::StreamSourceKey, _: u64) {
            self.accesses += 1;
        }
    }
    #[test]
    fn observed_pages_clock_exhaustion_rolls_back_before_provider_or_observer() {
        let project = hex_project::Project::new(48_000).unwrap();
        let revision = Arc::new(
            PreparedRevision::prepare(
                &project,
                &std::collections::BTreeMap::new(),
                Arc::new(crate::SineTable::new()),
                None,
            )
            .unwrap(),
        );
        let mut runtime = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        // Fault-inject the otherwise impractical end of a lifetime clock. No
        // production seam or new state/ownership behavior is introduced.
        runtime.performance_frame = u64::MAX - 1;
        let before = (
            runtime.performance_frame,
            runtime.arrangement_frame,
            runtime.route_state_generation,
            runtime.playing,
        );
        let owners = Arc::strong_count(&revision);
        let mut pages = NoPages { accesses: 0 };
        let mut output = [[9.0; 2]; 2];
        let mut observed = 0;
        assert_eq!(
            runtime.render_with_pages_and_post_node_observer(
                &mut output,
                &mut pages,
                Some(&mut |_, _, _: &[Frame]| observed += 1)
            ),
            Err(SessionPlaybackError::ArithmeticOverflow)
        );
        assert_eq!(output, [[0.0; 2]; 2]);
        assert_eq!(
            (
                runtime.performance_frame,
                runtime.arrangement_frame,
                runtime.route_state_generation,
                runtime.playing
            ),
            before
        );
        assert_eq!(pages.accesses, 0);
        assert_eq!(observed, 0);
        assert_eq!(Arc::strong_count(&revision), owners);
        assert!(
            !runtime.candidate_admission_open,
            "existing render-attempt admission closure remains"
        );
    }
}
