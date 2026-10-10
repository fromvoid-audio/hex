use super::{commands::transition_captures, *};
use crate::compile::PreparedAudioSource;
use crate::{MediaPages, MidiTiming, StreamedPageSpan, render_streamed_page_span};
use hex_project::LoopRegion;
use std::sync::Arc;

impl SessionPlayback {
    /// Render a complete block transactionally. A refusal after any chunk
    /// silences the requested block and leaves committed route/clock state as-is.
    pub fn render(&mut self, output: &mut [Frame]) -> Result<(), SessionPlaybackError> {
        self.candidate_admission_open = false;
        let contains_streamed_sources = if self.linear_arrangement_export {
            self.revision.arrangement().contains_streamed_sources()
        } else {
            self.revision.contains_streamed_sources()
        };
        if self.playing && !output.is_empty() && contains_streamed_sources {
            return Err(SessionPlaybackError::MediaPagesUnavailable);
        }
        self.render_internal::<dyn MediaPages>(output, None)
    }

    /// Render with a borrowed page view. Misses silence only the unavailable
    /// source span; other tracks and the performance clock continue.
    pub fn render_with_pages<P: MediaPages>(
        &mut self,
        output: &mut [Frame],
        pages: &mut P,
    ) -> Result<(), SessionPlaybackError> {
        self.candidate_admission_open = false;
        self.render_internal(output, Some(pages))
    }

    fn render_internal<P: MediaPages + ?Sized>(
        &mut self,
        output: &mut [Frame],
        pages: Option<&mut P>,
    ) -> Result<(), SessionPlaybackError> {
        output.fill([0.0; 2]);
        if output.is_empty() || !self.playing {
            return Ok(());
        }
        let result = self.render_playing(output, pages);
        if result.is_err() {
            output.fill([0.0; 2]);
            for runtime in self.mutable_instruments.iter_mut().flatten() {
                runtime.rollback_block();
            }
            for tail in self.tail_mutable_instruments.iter_mut().flatten() {
                tail.runtime.rollback_block();
            }
            self.return_effect_tails.rollback_block();
        }
        result
    }

    fn render_playing<P: MediaPages + ?Sized>(
        &mut self,
        output: &mut [Frame],
        mut pages: Option<&mut P>,
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
                    (entry.track_index, entry.tail_group, entry.order)
                });
            }
            let mut chunk_frames = (output.len() - output_offset).min(256);
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
            self.render_route_chunk(
                performance_frame,
                arrangement_frame,
                arrangement_loop_pass,
                &mut output[output_offset..chunk_end],
                pages.as_deref_mut(),
            )?;
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
            if self
                .mutable_instruments
                .get(index)
                .and_then(Option::as_ref)
                .is_some_and(|runtime| !runtime.speculative_is_quiescent())
            {
                return Err(SessionPlaybackError::MutableInstrumentActive);
            }
            if transition_captures(owner, pending.target) {
                self.capture_outgoing(index, owner, arrangement_frame, performance_frame)?;
                captured_release = true;
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
                let track = self.route_scratch[index].id;
                self.suppression_scratch
                    .retain(|entry| entry.track != track);
            }
        }
        Ok((captured_release, applied_route))
    }

    fn render_route_chunk<P: MediaPages + ?Sized>(
        &mut self,
        performance_frame: u64,
        arrangement_frame: u64,
        arrangement_loop_pass: u64,
        output: &mut [Frame],
        mut pages: Option<&mut P>,
    ) -> Result<(), SessionPlaybackError> {
        let tracks = self.revision.arrangement().tracks();
        for scratch in &mut self.return_scratch {
            scratch[..output.len()].fill([0.0; 2]);
        }
        let timing = timing_view_for(self.revision.session(), self.periodic_tempo, self.anchor);
        let table = self.revision.session().sine_table().as_ref();
        let mut release_cursor = 0;
        for index in 0..tracks.len() {
            let track = tracks
                .get(index)
                .ok_or(SessionPlaybackError::UnknownTrack)?;
            let mix = track.mix();
            let stateful_effects = self
                .device_chains
                .get(index)
                .is_some_and(|chain| !chain.is_empty());
            let mutable_active = self.mutable_instruments.get(index).is_some_and(|runtime| {
                runtime
                    .as_ref()
                    .is_some_and(|runtime| !runtime.committed_is_quiescent())
            });
            let buffered = stateful_effects || mutable_active || !track.sends().is_empty();
            let ordered = track.requires_ordered_processing();
            while self
                .release_scratch
                .get(release_cursor)
                .is_some_and(|entry| entry.track_index < index)
            {
                release_cursor += 1;
            }
            let first_release = release_cursor;
            while self
                .release_scratch
                .get(release_cursor)
                .is_some_and(|entry| entry.track_index == index)
            {
                release_cursor += 1;
            }
            let has_current_release = self.release_scratch[first_release..release_cursor]
                .iter()
                .any(|entry| entry.tail_group.is_none());
            let has_current_route = !matches!(self.route_scratch[index].owner, RouteOwner::Silence);
            let has_current_effect_tail = !has_current_route
                && !has_current_release
                && self
                    .device_chains
                    .get(index)
                    .is_some_and(|chain| !chain.committed_range_is_quiescent(0..chain.len()));
            if (buffered || ordered)
                && (has_current_route || has_current_release || has_current_effect_tail)
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
                            index,
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
                        self.device_chains[index]
                            .process_range(0..track.instrument_position(), render_output)
                            .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
                    } else {
                        track.apply_ordered_prefix(render_output);
                    }
                    for release in &self.release_scratch[first_release..release_cursor] {
                        if release.tail_group.is_none() {
                            render_release_raw(
                                release,
                                table,
                                self.revision.arrangement().shared_contexts(),
                                &self.retained_contexts,
                                performance_frame,
                                render_output,
                            )?;
                        }
                    }
                    render_route_source_chunk(
                        RouteSourceContext {
                            revision: &self.revision,
                            route: self.route_scratch[index],
                            historical_floor: self.historical_floor_for(index),
                            suppressions: &self.suppression_scratch,
                            index,
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
                    if stateful_effects {
                        let chain = self
                            .device_chains
                            .get_mut(index)
                            .ok_or(SessionPlaybackError::UnknownTrack)?;
                        let effect_count = chain.len();
                        chain
                            .process_range(track.instrument_position()..effect_count, render_output)
                            .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
                    } else {
                        track.apply_ordered_suffix(render_output);
                    }
                } else {
                    for release in &self.release_scratch[first_release..release_cursor] {
                        if release.tail_group.is_none() {
                            render_release_raw(
                                release,
                                table,
                                self.revision.arrangement().shared_contexts(),
                                &self.retained_contexts,
                                performance_frame,
                                render_output,
                            )?;
                        }
                    }
                    render_route_source_chunk(
                        RouteSourceContext {
                            revision: &self.revision,
                            route: self.route_scratch[index],
                            historical_floor: self.historical_floor_for(index),
                            suppressions: &self.suppression_scratch,
                            index,
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
                    let chain = self
                        .device_chains
                        .get_mut(index)
                        .ok_or(SessionPlaybackError::UnknownTrack)?;
                    chain.process_block(render_output);
                }
                for (frame_index, source) in render_output.iter().copied().enumerate() {
                    if track.sends().is_empty() {
                        mix.add_frame(&mut output[frame_index], source);
                    } else {
                        let contribution = mix.apply_frame(source);
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
                }
            } else {
                for release in &self.release_scratch[first_release..release_cursor] {
                    if release.tail_group.is_none() {
                        render_release(
                            release,
                            table,
                            self.revision.arrangement().shared_contexts(),
                            &self.retained_contexts,
                            performance_frame,
                            output,
                        )?;
                    }
                }
                render_route_source_chunk(
                    RouteSourceContext {
                        revision: &self.revision,
                        route: self.route_scratch[index],
                        historical_floor: self.historical_floor_for(index),
                        suppressions: &self.suppression_scratch,
                        index,
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
        // Removed stateless tracks retain their copied linear mix. Stateful
        // tails are grouped by their captured old runtime chain and old mix.
        let mut rendered_mutable_tails = [false; SESSION_TAIL_GROUP_CAPACITY];
        let mut rendered_tail_suffixes = [false; SESSION_TAIL_GROUP_CAPACITY];
        let mut tail_cursor = release_cursor;
        while tail_cursor < self.release_scratch.len() {
            let release = self
                .release_scratch
                .get(tail_cursor)
                .ok_or(SessionPlaybackError::ReleaseBankFull)?;
            let Some(group) = release.tail_group else {
                render_release(
                    release,
                    table,
                    self.revision.arrangement().shared_contexts(),
                    &self.retained_contexts,
                    performance_frame,
                    output,
                )?;
                tail_cursor += 1;
                continue;
            };
            let group_start = tail_cursor;
            while self
                .release_scratch
                .get(tail_cursor)
                .is_some_and(|entry| entry.tail_group == Some(group))
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
            chain.process_block(frames);
            if let Some(matrix) = bus.matrix {
                matrix.apply_block(frames);
            }
            if !bus.muted {
                for (destination, source) in output.iter_mut().zip(frames.iter()) {
                    destination[0] += source[0] * bus.gain;
                    destination[1] += source[1] * bus.gain;
                }
            }
        }
        self.return_effect_tails.render_chunk(output);
        Ok(())
    }
}

fn process_mutable_tail(
    tail: &mut super::MutableInstrumentTail,
    onsets: &mut MutableOnsetBuffer,
    events: &mut Vec<hex_lattice::NativeInstrumentEvent>,
    performance_frame: u64,
    output: &mut [Frame],
) -> Result<(), SessionPlaybackError> {
    onsets.clear();
    tail.runtime
        .process_segment(
            tail.track_id,
            performance_frame,
            performance_frame,
            output,
            onsets,
            &[],
            events,
        )
        .map_err(|_| SessionPlaybackError::MutableInstrumentRuntime)
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
    match routes
        .get(index)
        .ok_or(SessionPlaybackError::UnknownTrack)?
        .owner
    {
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
                .midi_clip(clip)
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
        .process_segment(
            routes[index].id,
            performance_frame,
            match routes[index].owner {
                RouteOwner::Arrangement => arrangement_frame,
                _ => performance_frame,
            },
            output,
            onsets,
            suppressions,
            events,
        )
        .map_err(SessionPlaybackError::from)
}

struct RouteSourceContext<'a, T> {
    revision: &'a PreparedRevision,
    route: super::TrackRoute,
    historical_floor: Option<BeatTime>,
    suppressions: &'a [MidiOccurrenceSuppression],
    index: usize,
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
        index,
        timing,
        table,
    } = context;
    let track = revision
        .arrangement()
        .tracks()
        .get(index)
        .ok_or(SessionPlaybackError::UnknownTrack)?;
    let source_mix = track.source_mix();
    match route.owner {
        RouteOwner::Arrangement => {
            let has_active_suppression = suppressions
                .iter()
                .any(|entry| entry.track == route.id && entry.until_frame > performance_frame);
            match stage {
                SourceStage::Whole => {
                    if has_active_suppression {
                        track.render_additive_with_suppressions_and_pages(
                            arrangement_frame,
                            performance_frame,
                            output,
                            suppressions,
                            pages.as_deref_mut(),
                        )
                    } else if let Some(page_provider) = pages.as_deref_mut() {
                        track.render_additive_with_pages(arrangement_frame, output, page_provider)
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
                .midi_clip(clip)
                .ok_or(SessionPlaybackError::UnknownClip)?;
            if prepared.track != route.id {
                return Err(SessionPlaybackError::ClipTrackMismatch);
            }
            if prepared.instrument_enabled() && !prepared.instrument_template().is_event_source() {
                let context = crate::MidiInstrumentRenderContext::new(launch, timing, table);
                let context = historical_floor.map_or(context, |floor| context.after_onset(floor));
                let context = context.with_suppressions(suppressions);
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
                        let sample = *frames
                            .get(source_index)
                            .ok_or(SessionPlaybackError::UnknownClip)?;
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

fn render_release(
    release: &super::ReleaseEntry,
    _table: &hex_devices::SineTable,
    contexts: &crate::shared_analytic::SharedAnalyticContextTable,
    retained: &[Option<Arc<crate::shared_analytic::SharedContextOwner>>],
    performance_frame: u64,
    output: &mut [Frame],
) -> Result<(), SessionPlaybackError> {
    for (offset, frame) in output.iter_mut().enumerate() {
        let relative =
            u64::try_from(offset).map_err(|_| SessionPlaybackError::ArithmeticOverflow)?;
        let absolute = performance_frame
            .checked_add(relative)
            .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
        if let Some(sample) = release.voice.sample_at(absolute, contexts, retained) {
            release.mix.add_frame(frame, sample);
        }
    }
    Ok(())
}

fn render_release_raw(
    release: &super::ReleaseEntry,
    _table: &hex_devices::SineTable,
    contexts: &crate::shared_analytic::SharedAnalyticContextTable,
    retained: &[Option<Arc<crate::shared_analytic::SharedContextOwner>>],
    performance_frame: u64,
    output: &mut [Frame],
) -> Result<(), SessionPlaybackError> {
    for (offset, frame) in output.iter_mut().enumerate() {
        let relative =
            u64::try_from(offset).map_err(|_| SessionPlaybackError::ArithmeticOverflow)?;
        let absolute = performance_frame
            .checked_add(relative)
            .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
        if let Some(sample) = release.voice.sample_at(absolute, contexts, retained) {
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

fn normalize_loop_cursor(
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
