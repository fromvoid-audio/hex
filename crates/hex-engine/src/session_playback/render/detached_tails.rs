//! Detached tail sends and mutable tail rendering.
use super::*;

/// Contribute a detached source to each retained post-fader send before
/// the ordinary Return owner processes its complete input once.
pub(super) fn add_detached_source_sends(
    sends: &crate::mutable_instrument::DetachedSourceSends,
    returns: &[crate::ReturnBus],
    scratch: &mut [[Frame; 256]],
    mix: crate::PreparedTrackMix,
    output: &[Frame],
) -> Result<(), SessionPlaybackError> {
    for (id, gain) in sends.iter() {
        let index = returns
            .iter()
            .position(|bus| bus.id == id)
            .ok_or(SessionPlaybackError::DeviceRuntime)?;
        let destination = scratch
            .get_mut(index)
            .ok_or(SessionPlaybackError::DeviceRuntime)?;
        for (target, source) in destination.iter_mut().zip(output.iter().copied()) {
            let contribution = mix.apply_frame(source);
            target[0] += contribution[0] * gain;
            target[1] += contribution[1] * gain;
        }
    }
    Ok(())
}

/// A retained suffix is post-fader. Grouped rendering adds it to its original
/// default route and, only with a retained graph proof, its sends and feeds.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_detached_tail(
    graph: Option<&crate::signal_graph::SignalGraph>,
    owner: Option<TrackId>,
    mix: crate::PreparedTrackMix,
    tail: &mut [Frame],
    output: &mut [Frame],
    offset: usize,
    capture: &mut Option<&mut dyn CaptureSink>,
    routed: &mut Option<&mut [Frame]>,
    routed_sends: &mut Option<&mut [Frame]>,
    allow_sends: bool,
) -> Result<(), SessionPlaybackError> {
    if let Some(destinations) = routed.as_deref_mut() {
        let owner = owner.ok_or(SessionPlaybackError::DeviceRuntime)?;
        let index = graph
            .and_then(|graph| graph.track_index(owner))
            .ok_or(SessionPlaybackError::UnknownTrack)?;
        let range = index * tail.len()..(index + 1) * tail.len();
        let destination = destinations
            .get_mut(range.clone())
            .ok_or(SessionPlaybackError::UnknownTrack)?;
        for (dst, &src) in destination.iter_mut().zip(tail.iter()) {
            mix.add_frame(dst, src);
        }
        if allow_sends {
            let sent = routed_sends
                .as_deref_mut()
                .and_then(|dst| dst.get_mut(range))
                .ok_or(SessionPlaybackError::UnknownTrack)?;
            for (dst, &src) in sent.iter_mut().zip(tail.iter()) {
                mix.add_frame(dst, src);
            }
        }
    } else {
        for (destination, source) in output.iter_mut().zip(tail.iter().copied()) {
            mix.add_frame(destination, source);
        }
        if let Some(capture) = capture.as_mut() {
            let owner = owner.ok_or(SessionPlaybackError::DeviceRuntime)?;
            for frame in tail.iter_mut() {
                *frame = mix.apply_frame(*frame);
            }
            capture.add(OutputCaptureId::Track(owner), offset, tail)?;
        }
    }
    Ok(())
}

pub(super) fn process_mutable_tail<T: MidiTiming + ?Sized>(
    tail: &mut super::MutableInstrumentTail,
    onsets: &mut MutableOnsetBuffer,
    events: &mut Vec<hex_lattice::NativeInstrumentEvent>,
    performance_frame: u64,
    output: &mut [Frame],
    timing: &T,
    playing: bool,
) -> Result<(), SessionPlaybackError> {
    let track = tail.track_id;
    tail.render_source(
        output,
        SessionPlaybackError::DeviceRuntime,
        |runtime, output| {
            onsets.clear();
            runtime
                .with_frozen_parameters(|runtime, values| {
                    if !playing && runtime.is_source_owner() {
                        let clock = runtime
                            .source_monitor_clock(
                                timing
                                    .frame_to_beat(performance_frame)
                                    .map_err(|_| SessionPlaybackError::TempoConversion)?
                                    .raw(),
                                timing
                                    .tempo_micro_bpm_at_frame(performance_frame)
                                    .ok_or(SessionPlaybackError::TempoConversion)?,
                                timing.sample_rate(),
                            )
                            .map_err(|error| error.render_error())?;
                        runtime
                            .process_frozen_tail_segment(
                                None,
                                performance_frame,
                                output,
                                onsets,
                                events,
                                Some(&clock),
                                values,
                            )
                            .map_err(|error| error.render_error())
                    } else {
                        runtime
                            .process_frozen_tail_segment(
                                Some(track),
                                performance_frame,
                                output,
                                onsets,
                                events,
                                Some(&InstrumentClock {
                                    timing,
                                    start: performance_frame,
                                }),
                                values,
                            )
                            .map_err(|error| error.render_error())
                    }
                })
                .map_err(|error| error.render_error())?
        },
    )
}

impl SessionPlayback {
    pub(super) fn render_detached_tails(
        &mut self,
        performance_frame: u64,
        output: &mut [Frame],
        frame_offset: usize,
        capture: &mut Option<&mut dyn CaptureSink>,
        routing: DetachedTailRouting<'_>,
        foreign: &mut dyn hex_lattice::ForeignProcessors,
    ) -> Result<(), SessionPlaybackError> {
        let DetachedTailRouting {
            mut routed,
            mut routed_sends,
        } = routing;
        let track_count = self.revision.arrangement().tracks().len();
        if self.detached_tail_group_limit == 0
            && self
                .release_scratch
                .last()
                .is_none_or(|release| release.location.track_index() < track_count)
        {
            return Ok(());
        }
        let timing = timing_view_for(self.revision.session(), self.periodic_tempo(), self.anchor);
        let table = self.revision.session().sine_table().as_ref();
        let graph = routed
            .as_ref()
            .and_then(|_| self.revision.arrangement().signal_graph.as_deref());
        // Removed stateless tracks retain their copied linear mix. Stateful
        // tails are grouped by their captured old runtime chain and old mix.
        let mut rendered_mutable_tails = [false; SESSION_TAIL_GROUP_CAPACITY];
        let mut rendered_tail_suffixes = [false; SESSION_TAIL_GROUP_CAPACITY];
        let mut tail_cursor = self
            .release_scratch
            .partition_point(|entry| entry.location.track_index() < track_count);
        while tail_cursor < self.release_scratch.len() {
            let release = self
                .release_scratch
                .get(tail_cursor)
                .ok_or(SessionPlaybackError::ReleaseBankFull)?;
            let Some(group) = release.location.tail_group() else {
                let mut captured = [[0.0; 2]; 256];
                if let Some(destinations) = routed.as_deref_mut() {
                    render_release_captured(
                        release,
                        table,
                        self.revision.arrangement().shared_contexts(),
                        &self.retained_contexts,
                        &self.analytic_continuations,
                        performance_frame,
                        self.playing,
                        &mut captured[..output.len()],
                        None,
                    )?;
                    let index = graph
                        .and_then(|graph| graph.track_index(release.track_id))
                        .ok_or(SessionPlaybackError::UnknownTrack)?;
                    let destination = destinations
                        .get_mut(index * output.len()..(index + 1) * output.len())
                        .ok_or(SessionPlaybackError::UnknownTrack)?;
                    compensated::add_main_track(destination, &captured[..output.len()]);
                } else {
                    render_release_captured(
                        release,
                        table,
                        self.revision.arrangement().shared_contexts(),
                        &self.retained_contexts,
                        &self.analytic_continuations,
                        performance_frame,
                        self.playing,
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
                    self.revision.arrangement().shared_contexts(),
                    &self.retained_contexts,
                    &self.analytic_continuations,
                    performance_frame,
                    self.playing,
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
                if !tail.speculative_is_quiescent() {
                    process_mutable_tail(
                        tail,
                        &mut self.mutable_onsets,
                        &mut self.mutable_events,
                        performance_frame,
                        tail_output,
                        &timing,
                        self.playing,
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
                    .process_range_with_foreign(
                        suffix_start..effect_count,
                        tail_output,
                        frame_offset,
                        None,
                        None,
                        foreign,
                    )
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
            if let Some(tail) = self.tail_mutable_instruments[group_index].as_ref() {
                add_detached_source_sends(
                    &tail.sends,
                    self.revision.arrangement().return_buses(),
                    &mut self.return_scratch,
                    mix,
                    tail_output,
                )?;
            }
            emit_detached_tail(
                graph,
                self.tail_track_ids[group_index],
                mix,
                tail_output,
                output,
                frame_offset,
                capture,
                &mut routed,
                &mut routed_sends,
                self.tail_route_sends[group_index],
            )?;
        }
        for group_index in 0..self.detached_tail_group_limit {
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
            if tail.powers_off_through_receiver() || tail.speculative_is_quiescent() {
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
                self.playing,
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
                    .process_range_with_foreign(
                        suffix_start..effect_count,
                        tail_output,
                        frame_offset,
                        None,
                        None,
                        foreign,
                    )
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
            add_detached_source_sends(
                &tail.sends,
                self.revision.arrangement().return_buses(),
                &mut self.return_scratch,
                mix,
                tail_output,
            )?;
            emit_detached_tail(
                graph,
                self.tail_track_ids[group_index],
                mix,
                tail_output,
                output,
                frame_offset,
                capture,
                &mut routed,
                &mut routed_sends,
                self.tail_route_sends[group_index],
            )?;
        }
        // Once an old source has stopped, its committed effect history still
        // owns output. Advance that suffix exactly once per render chunk using
        // real zero input, then keep its captured matrix and mix in the path.
        for group_index in 0..self.detached_tail_group_limit {
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
                .process_range_with_foreign(
                    suffix_start..effect_count,
                    tail_output,
                    frame_offset,
                    None,
                    None,
                    foreign,
                )
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
            emit_detached_tail(
                graph,
                self.tail_track_ids[group_index],
                mix,
                tail_output,
                output,
                frame_offset,
                capture,
                &mut routed,
                &mut routed_sends,
                self.tail_route_sends[group_index],
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_detached_source_sends_is_a_noop() {
        let sends = crate::mutable_instrument::DetachedSourceSends::default();
        let mut scratch = [[[0.0; 2]; 256]];
        add_detached_source_sends(
            &sends,
            &[],
            &mut scratch,
            crate::PreparedTrackMix::identity(),
            &[[0.5, -0.5]; 4],
        )
        .unwrap();
        assert_eq!(scratch[0][0], [0.0, 0.0]);
    }
}
