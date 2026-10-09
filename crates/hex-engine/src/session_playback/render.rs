use super::{commands::transition_captures, *};
use crate::compile::PreparedAudioSource;
use crate::{MediaPages, MidiTiming, StreamedPageSpan, render_streamed_page_span};
use hex_project::LoopRegion;

impl SessionPlayback {
    /// Render a complete block transactionally. A refusal after any chunk
    /// silences the requested block and leaves committed route/clock state as-is.
    pub fn render(&mut self, output: &mut [Frame]) -> Result<(), SessionPlaybackError> {
        if self.playing && !output.is_empty() && self.revision.contains_streamed_sources() {
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
        for chain in self.tail_groups.iter_mut().flatten() {
            chain.begin_block();
        }
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
        let mut output_offset = 0;
        let mut route_changed = false;
        let region = self
            .revision
            .arrangement()
            .loop_region()
            .filter(|loop_region| loop_region.enabled());

        while output_offset < output.len() {
            if let Some(region) = region {
                arrangement_frame = normalize_loop_cursor(arrangement_frame, region)?;
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
                    arrangement_frame = normalize_loop_cursor(arrangement_frame, region)?;
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
                arrangement_frame = normalize_loop_cursor(arrangement_frame, region)?;
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
        for chain in self.tail_groups.iter_mut().flatten() {
            chain.commit_block();
        }
        std::mem::swap(&mut self.routes, &mut self.route_scratch);
        std::mem::swap(&mut self.releases, &mut self.release_scratch);
        std::mem::swap(&mut self.suppressions, &mut self.suppression_scratch);
        self.release_order = self.release_order_scratch;
        self.performance_frame = performance_frame;
        self.arrangement_frame = arrangement_frame;
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
        output: &mut [Frame],
        mut pages: Option<&mut P>,
    ) -> Result<(), SessionPlaybackError> {
        let tracks = self.revision.arrangement().tracks();
        let timing = timing_view_for(self.revision.session(), self.periodic_tempo, self.anchor);
        let table = self.revision.session().sine_table().as_ref();
        let mut release_cursor = 0;
        for index in 0..tracks.len() {
            let track = tracks
                .get(index)
                .ok_or(SessionPlaybackError::UnknownTrack)?;
            let mix = track.mix();
            let stateful = self
                .device_chains
                .get(index)
                .is_some_and(|chain| !chain.is_empty());
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
            if (stateful || ordered) && (has_current_route || has_current_release) {
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
                    if stateful {
                        self.device_chains[index]
                            .process_range(0..track.instrument_position(), render_output)
                            .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
                    } else {
                        track.apply_ordered_prefix(render_output);
                    }
                    for release in &self.release_scratch[first_release..release_cursor] {
                        if release.tail_group.is_none() {
                            render_release_raw(release, table, performance_frame, render_output)?;
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
                    if stateful {
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
                            render_release_raw(release, table, performance_frame, render_output)?;
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
                for (destination, source) in output.iter_mut().zip(render_output.iter().copied()) {
                    mix.add_frame(destination, source);
                }
            } else {
                for release in &self.release_scratch[first_release..release_cursor] {
                    if release.tail_group.is_none() {
                        render_release(release, table, performance_frame, output)?;
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
        let mut tail_cursor = release_cursor;
        while tail_cursor < self.release_scratch.len() {
            let release = self
                .release_scratch
                .get(tail_cursor)
                .ok_or(SessionPlaybackError::ReleaseBankFull)?;
            let Some(group) = release.tail_group else {
                render_release(release, table, performance_frame, output)?;
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
                render_release_raw(release, table, performance_frame, tail_output)?;
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
        Ok(())
    }
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
            if prepared.instrument_enabled() {
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
    performance_frame: u64,
    output: &mut [Frame],
) -> Result<(), SessionPlaybackError> {
    for (offset, frame) in output.iter_mut().enumerate() {
        let relative =
            u64::try_from(offset).map_err(|_| SessionPlaybackError::ArithmeticOverflow)?;
        let absolute = performance_frame
            .checked_add(relative)
            .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
        if let Some(sample) = release.voice.sample_at(absolute) {
            release.mix.add_frame(frame, sample);
        }
    }
    Ok(())
}

fn render_release_raw(
    release: &super::ReleaseEntry,
    _table: &hex_devices::SineTable,
    performance_frame: u64,
    output: &mut [Frame],
) -> Result<(), SessionPlaybackError> {
    for (offset, frame) in output.iter_mut().enumerate() {
        let relative =
            u64::try_from(offset).map_err(|_| SessionPlaybackError::ArithmeticOverflow)?;
        let absolute = performance_frame
            .checked_add(relative)
            .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
        if let Some(sample) = release.voice.sample_at(absolute) {
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

fn normalize_loop_cursor(frame: u64, region: LoopRegion) -> Result<u64, SessionPlaybackError> {
    if frame < region.end() {
        return Ok(frame);
    }
    let length = region
        .end()
        .checked_sub(region.start())
        .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
    if length == 0 {
        return Err(SessionPlaybackError::ArithmeticOverflow);
    }
    Ok(region.start() + (frame - region.start()) % length)
}
