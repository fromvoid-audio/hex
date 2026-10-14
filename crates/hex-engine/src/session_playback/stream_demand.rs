//! Pure next-chunk planning: no route application, voice admission or DSP work.
use super::*;
use crate::{Invalid, StreamChunkWindow, StreamPageExtent, compile::PreparedAudioSource};

pub(super) struct HandoffDemand<'a> {
    pub maximum_frames: usize,
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
        visit: &mut impl FnMut(StreamPageExtent) -> Result<(), Invalid>,
    ) -> Result<StreamChunkWindow, SessionPlaybackError> {
        if self.linear_arrangement_export || previous.linear_arrangement_export {
            return Err(SessionPlaybackError::InvalidCommand);
        }
        self.require_fresh_candidate()?;
        let mut demand = HandoffDemand {
            maximum_frames,
            visit,
            window: None,
        };
        self.inherit_state_with_demand(previous, Some(&mut demand))?;
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
        visit: &mut impl FnMut(StreamPageExtent) -> Result<(), Invalid>,
    ) -> Result<StreamChunkWindow, Invalid> {
        self.visit_stream_chunk_at(
            &self.routes,
            self.performance_frame,
            self.arrangement_frame,
            self.route_state_generation,
            self.arrangement_loop_pass,
            self.playing,
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
        let mut window = StreamChunkWindow {
            performance_frame,
            arrangement_frame,
            route_generation,
            frames: if playing { maximum_frames.min(256) } else { 0 },
        };
        if window.frames == 0 {
            return Ok(window);
        }
        let region = (!self.linear_arrangement_export)
            .then(|| self.revision.arrangement().loop_region())
            .flatten()
            .filter(|region| region.enabled());
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
                        .typing
                        .as_ref()
                        .is_some_and(|typing| typing.arm().target().track == route.id)
                        || self
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
                    if buffered
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
                    if let PreparedAudioSource::Streamed(source) = clip.source() {
                        if let Some(elapsed) = performance_frame.checked_sub(launch_frame) {
                            if let Some(start) = clip
                                .source_start
                                .checked_add(elapsed)
                                .filter(|start| *start < clip.source_end)
                            {
                                let count = window.frames.min(
                                    usize::try_from(clip.source_end - start).unwrap_or(usize::MAX),
                                );
                                crate::stream_demand::visit_page_range(
                                    source, start, count, visit,
                                )?;
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
