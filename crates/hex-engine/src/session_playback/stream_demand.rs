//! Pure next-chunk planning adapted from published177 8d444c06.
//! No route application, owner handoff, transport commit or DSP work.
use super::*;
use crate::{Invalid, StreamChunkWindow, StreamPageExtent, compile::PreparedAudioSource};

pub(super) struct HandoffDemand<'a> {
    pub ordinary: bool,
    pub maximum_frames: usize,
    pub playing: Option<bool>,
    pub visit: &'a mut dyn FnMut(StreamPageExtent) -> Result<(), Invalid>,
    pub window: Option<StreamChunkWindow>,
}

impl SessionPlayback {
    /// Repeat the actual handoff's pure planning and enumerate its next chunk.
    /// Does not consume candidate admission or transfer any owner. A later
    /// commit must revalidate against the actual current predecessor.
    pub fn preview_stream_handoff(
        &mut self,
        previous: &mut Self,
        maximum_frames: usize,
        playing: Option<bool>,
        visit: &mut impl FnMut(StreamPageExtent) -> Result<(), Invalid>,
    ) -> Result<StreamChunkWindow, SessionPlaybackError> {
        self.preview_stream_handoff_with_scope(previous, maximum_frames, playing, visit, false)
    }
    pub(crate) fn preview_stream_handoff_with_scope(
        &mut self,
        previous: &mut Self,
        maximum_frames: usize,
        playing: Option<bool>,
        visit: &mut impl FnMut(StreamPageExtent) -> Result<(), Invalid>,
        ordinary: bool,
    ) -> Result<StreamChunkWindow, SessionPlaybackError> {
        if self.linear_arrangement_export
            || previous.linear_arrangement_export
            || (!ordinary
                && (self.compensation.is_some()
                    || previous.compensation.is_some()
                    || self.computer_keyboard.is_some()
                    || previous.computer_keyboard.is_some()))
        {
            return Err(SessionPlaybackError::InvalidCommand);
        }
        self.require_fresh_candidate()?;
        let mut demand = HandoffDemand {
            ordinary,
            maximum_frames,
            playing,
            visit,
            window: None,
        };
        self.inherit_state_planned(previous, None, Some(&mut demand))?;
        demand
            .window
            .ok_or(SessionPlaybackError::ArithmeticOverflow)
    }

    /// Enumerate the next chunk's streamed page coverage without mutation.
    /// Visitors borrow metadata only and must use preallocated storage on audio.
    /// A visitor refusal leaves this runtime unchanged; partial visitor storage
    /// is speculative and must be discarded. Paused/empty demand has width zero.
    pub fn visit_next_stream_chunk(
        &self,
        maximum_frames: usize,
        playing: Option<bool>,
        visit: &mut impl FnMut(StreamPageExtent) -> Result<(), Invalid>,
    ) -> Result<StreamChunkWindow, Invalid> {
        self.visit_stream_chunk_at(
            &self.routes,
            self.performance_frame,
            self.arrangement_frame,
            self.route_state_generation,
            self.arrangement_loop_pass,
            playing.unwrap_or(self.playing),
            maximum_frames,
            visit,
        )
    }

    #[allow(clippy::too_many_arguments)] // Borrowed applied/planned clocks, no ownership snapshot.
    pub(super) fn visit_stream_chunk_at(
        &self,
        routes: &[TrackRoute],
        performance_frame: u64,
        arrangement_frame: u64,
        route_generation: u64,
        loop_pass: u64,
        playing: bool,
        maximum_frames: usize,
        visit: &mut impl FnMut(StreamPageExtent) -> Result<(), Invalid>,
    ) -> Result<StreamChunkWindow, Invalid> {
        self.visit_stream_chunk_in_scope(
            false,
            routes,
            performance_frame,
            arrangement_frame,
            route_generation,
            loop_pass,
            playing,
            maximum_frames,
            visit,
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn visit_stream_chunk_in_scope(
        &self,
        ordinary: bool,
        routes: &[TrackRoute],
        performance_frame: u64,
        arrangement_frame: u64,
        route_generation: u64,
        loop_pass: u64,
        playing: bool,
        maximum_frames: usize,
        visit: &mut impl FnMut(StreamPageExtent) -> Result<(), Invalid>,
    ) -> Result<StreamChunkWindow, Invalid> {
        if !ordinary && (self.compensation.is_some() || self.computer_keyboard.is_some()) {
            return Err(Invalid("stream demand requires a closed audio runtime"));
        }
        self.visit_stream_chunk_sources_at(
            routes,
            performance_frame,
            arrangement_frame,
            route_generation,
            loop_pass,
            playing,
            maximum_frames,
            visit,
        )
    }
    /// Ordinary prepared playback retains its existing keyboard and compensated
    /// render owners. Paused monitoring reads no clip pages. Pre-Main latency
    /// and typed routes use the renderer's one-frame source quantum; Main-only
    /// latency and retained Main history keep upstream source work batched.
    /// Control consumers may inspect the same exact ordinary source quantum.
    /// This pure metadata visit establishes no native page/readiness authority.
    pub fn visit_ordinary_stream_chunk(
        &self,
        maximum_frames: usize,
        playing: Option<bool>,
        visit: &mut impl FnMut(StreamPageExtent) -> Result<(), Invalid>,
    ) -> Result<StreamChunkWindow, Invalid> {
        self.visit_stream_chunk_sources_at(
            &self.routes,
            self.performance_frame,
            self.arrangement_frame,
            self.route_state_generation,
            self.arrangement_loop_pass,
            playing.unwrap_or(self.playing),
            maximum_frames,
            visit,
        )
    }
    /// Visit the complete physical callback's source demand without advancing
    /// transport or processing. The caller must retain the union of visited
    /// extents before rendering any part of the callback. Route and loop
    /// transitions are simulated on stack-owned copies; no DSP state is copied.
    /// Muted arrangement tracks are visited conservatively when a compensated
    /// graph, mutable instrument or live/shared input can need their raw source
    /// during the callback. This may require more pages than next-chunk demand.
    pub fn visit_ordinary_stream_callback(
        &self,
        maximum_frames: usize,
        playing: Option<bool>,
        visit: &mut impl FnMut(StreamPageExtent) -> Result<(), Invalid>,
    ) -> Result<StreamChunkWindow, Invalid> {
        let playing = playing.unwrap_or(self.playing);
        let mut result = StreamChunkWindow {
            performance_frame: self.performance_frame,
            arrangement_frame: self.arrangement_frame,
            route_generation: self.route_state_generation,
            frames: if playing { maximum_frames } else { 0 },
        };
        if result.frames == 0 {
            return Ok(result);
        }
        let frame_count = u64::try_from(result.frames)
            .map_err(|_| Invalid("stream callback frame count overflow"))?;
        self.performance_frame
            .checked_add(frame_count)
            .ok_or(Invalid("stream callback performance clock overflow"))?;
        if self.routes.len() > crate::MAX_SESSION_ROUTE_TRACKS {
            return Err(Invalid("stream callback route capacity exceeded"));
        }
        let Some(first) = self.routes.first().copied() else {
            return Ok(result);
        };
        let mut scratch = [first; crate::MAX_SESSION_ROUTE_TRACKS];
        let routes = &mut scratch[..self.routes.len()];
        routes.copy_from_slice(&self.routes);
        let region = self.active_loop_region();
        let mut performance = self.performance_frame;
        let mut arrangement = self.arrangement_frame;
        let mut loop_pass = self.arrangement_loop_pass;
        let mut remaining = result.frames;
        while remaining != 0 {
            if let Some(region) = region {
                (arrangement, loop_pass) =
                    render::normalize_loop_cursor(arrangement, region, loop_pass)
                        .map_err(|error| Invalid(error.diagnostic_message()))?;
            }
            if remaining == result.frames {
                result.arrangement_frame = arrangement;
            }
            for route in routes.iter_mut() {
                if let Some(pending) = route.pending.filter(|pending| pending.frame <= performance)
                {
                    route.owner = self
                        .owner_for_target(pending.target, pending.beat, performance)
                        .map_err(|error| Invalid(error.diagnostic_message()))?;
                    route.pending = None;
                }
            }
            let window = self.visit_stream_chunk_sources_with_scope(
                routes,
                performance,
                arrangement,
                self.route_state_generation,
                loop_pass,
                true,
                remaining,
                true,
                visit,
            )?;
            if window.frames == 0 {
                return Err(Invalid("stream callback demand made no progress"));
            }
            let frames = u64::try_from(window.frames)
                .map_err(|_| Invalid("stream callback frame count overflow"))?;
            performance = performance
                .checked_add(frames)
                .ok_or(Invalid("stream callback performance clock overflow"))?;
            arrangement = window
                .arrangement_frame
                .checked_add(frames)
                .ok_or(Invalid("stream callback arrangement clock overflow"))?;
            remaining -= window.frames;
        }
        Ok(result)
    }
    /// Constructor runtimes keep the same closed-family restrictions as their
    /// next-chunk demand query while planning a full physical callback.
    pub fn visit_stream_callback(
        &self,
        maximum_frames: usize,
        playing: Option<bool>,
        visit: &mut impl FnMut(StreamPageExtent) -> Result<(), Invalid>,
    ) -> Result<StreamChunkWindow, Invalid> {
        if self.compensation.is_some() || self.computer_keyboard.is_some() {
            return Err(Invalid("stream demand requires a closed audio runtime"));
        }
        self.visit_ordinary_stream_callback(maximum_frames, playing, visit)
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn visit_stream_chunk_sources_at(
        &self,
        routes: &[TrackRoute],
        performance_frame: u64,
        arrangement_frame: u64,
        route_generation: u64,
        loop_pass: u64,
        playing: bool,
        maximum_frames: usize,
        visit: &mut impl FnMut(StreamPageExtent) -> Result<(), Invalid>,
    ) -> Result<StreamChunkWindow, Invalid> {
        self.visit_stream_chunk_sources_with_scope(
            routes,
            performance_frame,
            arrangement_frame,
            route_generation,
            loop_pass,
            playing,
            maximum_frames,
            false,
            visit,
        )
    }
    #[allow(clippy::too_many_arguments)] // Exact clocks and borrowed speculative route snapshot.
    fn visit_stream_chunk_sources_with_scope(
        &self,
        routes: &[TrackRoute],
        performance_frame: u64,
        arrangement_frame: u64,
        route_generation: u64,
        loop_pass: u64,
        playing: bool,
        maximum_frames: usize,
        whole_callback: bool,
        visit: &mut impl FnMut(StreamPageExtent) -> Result<(), Invalid>,
    ) -> Result<StreamChunkWindow, Invalid> {
        let mut window = StreamChunkWindow {
            performance_frame,
            arrangement_frame,
            route_generation,
            frames: if playing {
                maximum_frames.min(
                    if !whole_callback
                        && self
                            .compensation
                            .as_ref()
                            .is_some_and(|runtime| runtime.graph.needs_upstream_frame_routing())
                    {
                        1
                    } else {
                        256
                    },
                )
            } else {
                0
            },
        };
        if window.frames == 0 {
            return Ok(window);
        }
        let region = self.active_loop_region_at(arrangement_frame, loop_pass);
        if let Some(region) = region {
            window.arrangement_frame =
                render::normalize_loop_cursor(window.arrangement_frame, region, loop_pass)
                    .map_err(|error| Invalid(error.diagnostic_message()))?
                    .0;
            let until = if window.arrangement_frame < region.start() {
                region.start() - window.arrangement_frame
            } else {
                region.end() - window.arrangement_frame
            };
            window.frames = window
                .frames
                .min(usize::try_from(until).unwrap_or(usize::MAX));
        }
        if let Some(next) = routes
            .iter()
            .filter_map(|route| route.pending)
            .filter(|pending| pending.frame > performance_frame)
            .map(|pending| pending.frame)
            .min()
        {
            window.frames = window
                .frames
                .min(usize::try_from(next - performance_frame).unwrap_or(usize::MAX));
        }
        performance_frame
            .checked_add(window.frames as u64)
            .ok_or(Invalid("stream demand performance clock overflow"))?;
        window
            .arrangement_frame
            .checked_add(window.frames as u64)
            .ok_or(Invalid("stream demand arrangement clock overflow"))?;
        for (index, route) in routes.iter().enumerate() {
            let owner = if let Some(pending) = route
                .pending
                .filter(|pending| pending.frame <= performance_frame)
            {
                self.owner_for_target(pending.target, pending.beat, performance_frame)
                    .map_err(|error| Invalid(error.diagnostic_message()))?
            } else {
                route.owner
            };
            match owner {
                RouteOwner::Arrangement => {
                    let track = self
                        .revision
                        .arrangement()
                        .tracks()
                        .get(index)
                        .ok_or(Invalid("stream demand track missing"))?;
                    // Buffered/ordered paths read raw audio even when postmix is muted.
                    let buffered = self
                        .device_chains
                        .get(index)
                        .is_some_and(|chain| !chain.is_empty())
                        || self
                            .mutable_instruments
                            .get(index)
                            .and_then(Option::as_ref)
                            .is_some_and(|runtime| !runtime.committed_is_quiescent())
                        || !track.sends().is_empty()
                        || track.has_gain_automation()
                        || (self.device_metering && track.instrument_id.is_some());
                    let may_need_raw = whole_callback
                        && (self.compensation.is_some()
                            || self
                                .mutable_instruments
                                .get(index)
                                .and_then(Option::as_ref)
                                .is_some()
                            || self.revision.source_routing().is_shared(index)
                            || self.computer_keyboard.as_ref().is_some_and(|keyboard| {
                                keyboard.is_source_input() || keyboard.track() == route.id
                            })
                            || self
                                .current_sampler_keyboards
                                .iter()
                                .any(|keyboard| keyboard.track() == route.id));
                    if may_need_raw
                        || buffered
                        || track.requires_ordered_processing()
                        || !track.source_mix().muted()
                    {
                        track.visit_stream_demand(
                            window.arrangement_frame,
                            window.frames,
                            visit,
                        )?;
                    }
                }
                RouteOwner::SessionAudio { clip, launch_frame } => {
                    let clip = self
                        .revision
                        .session()
                        .audio_clip(clip)
                        .ok_or(Invalid("stream demand clip missing"))?;
                    if !clip.active {
                        continue;
                    }
                    if let PreparedAudioSource::Streamed(source) = clip.source() {
                        if let Some(elapsed) = performance_frame.checked_sub(launch_frame) {
                            if let Some(start) = clip.source_frame(elapsed) {
                                let count = window.frames.min(
                                    usize::try_from(clip.playback_end() - start)
                                        .unwrap_or(usize::MAX),
                                );
                                crate::stream_demand::visit_page_range(
                                    source, start, count, visit,
                                )?;
                                // Later complete cycles demand the same pages.
                                // One suffix and one cycle prefix cover the union,
                                // independent of block length or cycle density.
                                if let Some(range) = clip.loop_range {
                                    let rest = (window.frames - count).min(
                                        usize::try_from(range.end() - range.start())
                                            .unwrap_or(usize::MAX),
                                    );
                                    if rest > 0 {
                                        crate::stream_demand::visit_page_range(
                                            source,
                                            range.start(),
                                            rest,
                                            visit,
                                        )?;
                                    }
                                }
                            }
                        }
                    }
                }
                RouteOwner::Session { .. } | RouteOwner::Silence => {}
            }
        }
        Ok(window)
    }
}
