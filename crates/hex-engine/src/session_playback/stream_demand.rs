//! Pure next-chunk planning adapted from published177 8d444c06.
//! No route application, owner handoff, transport commit or DSP work.
use super::*;
use crate::{Invalid, StreamChunkWindow, StreamResourceExtent, compile::PreparedAudioSource};

pub(super) struct HandoffDemand<'a> {
    pub ordinary: bool,
    pub maximum_frames: usize,
    pub playing: Option<bool>,
    pub visit: &'a mut dyn FnMut(StreamResourceExtent) -> Result<(), Invalid>,
    pub window: Option<StreamChunkWindow>,
}

/// The history that will own the planned render frontier. A seek and a fresh
/// compensation epoch start empty; a copy/swap handoff reads the donor history.
#[derive(Clone, Copy)]
pub(super) enum ScalarOriginHistory<'a> {
    Current(&'a SessionPlayback),
    Empty,
}

impl SessionPlayback {
    /// This is derived from the retained runtime graph, not a caller-declared
    /// readiness mode. Serial Main-only latency reads the ordinary source
    /// frontier for Arrangement and Session audio. Flat audio-only tracks may
    /// use upstream compensation with closed native effects when Session audio
    /// is absent. Instruments, foreign effects, routed upstream graphs and
    /// keyboard remain excluded from these compensated extensions.
    pub(super) fn supports_closed_stream_demand(&self) -> bool {
        self.computer_keyboard.is_none()
            && self.compensation.as_ref().is_none_or(|runtime| {
                runtime.graph.supports_closed_group_stream()
                    || (runtime.graph.independent_main_routes()
                        && ((!runtime.graph.needs_upstream_frame_routing()
                            && self.revision.session().audio_clips().is_empty())
                            || ((!runtime.graph.needs_upstream_frame_routing()
                                || self.revision.session().audio_clips().is_empty())
                                && self.revision.session().midi_clips().is_empty()
                                && runtime.graph.tracks.iter().enumerate().all(|(index, _)| {
                                    runtime.graph.resident_main_alignment(index).is_some()
                                })
                                && self
                                    .revision
                                    .arrangement()
                                    .tracks()
                                    .iter()
                                    .flat_map(|track| track.device_chain.iter())
                                    .chain(self.revision.arrangement().main_device_chain.iter())
                                    .all(|device| {
                                        crate::native_effect_storage::is_closed_type(
                                            device.device_type,
                                        ) && !matches!(
                                            device.definition,
                                            Some(hex_lattice::PreparedProcessorFactory::Foreign(_))
                                        )
                                    })
                                && self.revision.arrangement().tracks().iter().all(|track| {
                                    track.instrument_id.is_none()
                                        && track.instrument.is_none()
                                        && track.foreign_instrument.is_none()
                                        && track.midi_source_index.is_none()
                                }))))
            })
    }

    /// Repeat the actual handoff's pure planning and enumerate its next chunk.
    /// Does not consume candidate admission or transfer any owner. A later
    /// commit must revalidate against the actual current predecessor.
    pub fn preview_stream_handoff(
        &mut self,
        previous: &mut Self,
        maximum_frames: usize,
        playing: Option<bool>,
        visit: &mut impl FnMut(StreamResourceExtent) -> Result<(), Invalid>,
    ) -> Result<StreamChunkWindow, SessionPlaybackError> {
        self.preview_stream_handoff_with_scope(previous, maximum_frames, playing, visit, false)
    }

    /// Preview the next stream chunk and the handoff selected for this exact
    /// predecessor. The report is observational: the eventual handoff must
    /// revalidate after any intervening transport, route, or source change.
    pub fn preview_stream_handoff_with_report(
        &mut self,
        previous: &mut Self,
        maximum_frames: usize,
        playing: Option<bool>,
        visit: &mut impl FnMut(StreamResourceExtent) -> Result<(), Invalid>,
    ) -> Result<(StreamChunkWindow, SessionHandoffReport), SessionPlaybackError> {
        self.preview_stream_handoff_with_policy_and_report(
            previous,
            maximum_frames,
            playing,
            visit,
            false,
            false,
        )
    }
    pub(crate) fn preview_stream_handoff_with_scope(
        &mut self,
        previous: &mut Self,
        maximum_frames: usize,
        playing: Option<bool>,
        visit: &mut impl FnMut(StreamResourceExtent) -> Result<(), Invalid>,
        ordinary: bool,
    ) -> Result<StreamChunkWindow, SessionPlaybackError> {
        self.preview_stream_handoff_with_policy(
            previous,
            maximum_frames,
            playing,
            visit,
            ordinary,
            false,
        )
    }

    pub(crate) fn preview_stream_handoff_with_policy(
        &mut self,
        previous: &mut Self,
        maximum_frames: usize,
        playing: Option<bool>,
        visit: &mut impl FnMut(StreamResourceExtent) -> Result<(), Invalid>,
        ordinary: bool,
        fenced_restart: bool,
    ) -> Result<StreamChunkWindow, SessionPlaybackError> {
        self.preview_stream_handoff_with_policy_and_report(
            previous,
            maximum_frames,
            playing,
            visit,
            ordinary,
            fenced_restart,
        )
        .map(|(window, _)| window)
    }

    pub(crate) fn preview_stream_handoff_with_policy_and_report(
        &mut self,
        previous: &mut Self,
        maximum_frames: usize,
        playing: Option<bool>,
        visit: &mut impl FnMut(StreamResourceExtent) -> Result<(), Invalid>,
        ordinary: bool,
        fenced_restart: bool,
    ) -> Result<(StreamChunkWindow, SessionHandoffReport), SessionPlaybackError> {
        self.preview_stream_handoff_with_continuation_and_report(
            previous,
            maximum_frames,
            playing,
            visit,
            ordinary,
            fenced_restart,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)] // Preserve the actual source, preview scope and exact borrowed matcher.
    pub(crate) fn preview_stream_handoff_with_continuation_and_report(
        &mut self,
        previous: &mut Self,
        maximum_frames: usize,
        playing: Option<bool>,
        visit: &mut impl FnMut(StreamResourceExtent) -> Result<(), Invalid>,
        ordinary: bool,
        fenced_restart: bool,
        continuation: Option<&hex_lattice::ForeignConfigurationContinuation<'_>>,
    ) -> Result<(StreamChunkWindow, SessionHandoffReport), SessionPlaybackError> {
        if self.linear_arrangement_export
            || previous.linear_arrangement_export
            || (!ordinary
                && (!self.supports_closed_stream_demand()
                    || !previous.supports_closed_stream_demand()))
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
        let report = self.inherit_state_planned(
            previous,
            None,
            Some(&mut demand),
            true,
            fenced_restart,
            continuation,
        )?;
        let window = demand
            .window
            .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
        Ok((window, report))
    }

    /// Enumerate the next chunk's streamed page coverage without mutation.
    /// Visitors borrow metadata only and must use preallocated storage on audio.
    /// A visitor refusal leaves this runtime unchanged; partial visitor storage
    /// is speculative and must be discarded. Paused/empty demand has width zero.
    /// Linear offline runtimes include compensated paths; interactive closed
    /// demand retains its existing restricted admission.
    pub fn visit_next_stream_chunk(
        &self,
        maximum_frames: usize,
        playing: Option<bool>,
        visit: &mut impl FnMut(StreamResourceExtent) -> Result<(), Invalid>,
    ) -> Result<StreamChunkWindow, Invalid> {
        self.visit_stream_chunk_in_scope(
            self.linear_arrangement_export,
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
        visit: &mut impl FnMut(StreamResourceExtent) -> Result<(), Invalid>,
    ) -> Result<StreamChunkWindow, Invalid> {
        self.visit_stream_chunk_with_history(
            ordinary,
            routes,
            performance_frame,
            arrangement_frame,
            route_generation,
            loop_pass,
            playing,
            maximum_frames,
            visit,
            ScalarOriginHistory::Current(self),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn visit_stream_chunk_with_history(
        &self,
        ordinary: bool,
        routes: &[TrackRoute],
        performance_frame: u64,
        arrangement_frame: u64,
        route_generation: u64,
        loop_pass: u64,
        playing: bool,
        maximum_frames: usize,
        visit: &mut impl FnMut(StreamResourceExtent) -> Result<(), Invalid>,
        history: ScalarOriginHistory<'_>,
    ) -> Result<StreamChunkWindow, Invalid> {
        if !ordinary && !self.supports_closed_stream_demand() {
            return Err(Invalid("stream demand requires a closed audio runtime"));
        }
        self.visit_stream_chunk_sources_with_history(
            routes,
            performance_frame,
            arrangement_frame,
            route_generation,
            loop_pass,
            playing,
            maximum_frames,
            visit,
            history,
        )
    }
    /// Ordinary prepared playback retains its existing keyboard and compensated
    /// render owners. Paused monitoring reads no clip pages. Running playback
    /// reports a bounded source window while compensated routing remains per-frame.
    /// Control consumers may inspect the same exact ordinary source quantum.
    /// This pure metadata visit establishes no native page/readiness authority.
    pub fn visit_ordinary_stream_chunk(
        &self,
        maximum_frames: usize,
        playing: Option<bool>,
        visit: &mut impl FnMut(StreamResourceExtent) -> Result<(), Invalid>,
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
        visit: &mut impl FnMut(StreamResourceExtent) -> Result<(), Invalid>,
    ) -> Result<StreamChunkWindow, Invalid> {
        self.visit_stream_chunk_sources_with_history(
            routes,
            performance_frame,
            arrangement_frame,
            route_generation,
            loop_pass,
            playing,
            maximum_frames,
            visit,
            ScalarOriginHistory::Current(self),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn visit_stream_chunk_sources_with_history(
        &self,
        routes: &[TrackRoute],
        performance_frame: u64,
        arrangement_frame: u64,
        route_generation: u64,
        loop_pass: u64,
        playing: bool,
        maximum_frames: usize,
        visit: &mut impl FnMut(StreamResourceExtent) -> Result<(), Invalid>,
        history: ScalarOriginHistory<'_>,
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
        let region = self.active_loop_region_at(arrangement_frame);
        if let Some(region) = region {
            (window.arrangement_frame, _) =
                render::normalize_loop_cursor(window.arrangement_frame, region, loop_pass)
                    .map_err(|error| Invalid(error.diagnostic_message()))?;
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
        let raw_audio = self
            .compensation
            .as_ref()
            .is_some_and(|runtime| runtime.graph.needs_upstream_frame_routing());
        for (index, route) in routes.iter().enumerate() {
            // The per-frame audio stage exits before reading either Arrangement
            // or Session pages when this retained track disables local content.
            if raw_audio
                && !self
                    .revision
                    .arrangement()
                    .tracks()
                    .get(index)
                    .ok_or(Invalid("stream demand track missing"))?
                    .audio_content_enabled()
            {
                continue;
            }
            let owner = if let Some(pending) = route
                .pending
                .filter(|pending| pending.frame <= performance_frame)
            {
                let next = self
                    .owner_for_target(pending.target, pending.beat, performance_frame)
                    .map_err(|error| Invalid(error.diagnostic_message()))?;
                // The owner left at this frame fades out over the window.
                if let RouteOwner::SessionAudio { clip, launch_frame } = route.owner
                    && next != route.owner
                {
                    let fade = usize::try_from(self.audio_release_fade()).unwrap_or(usize::MAX);
                    self.visit_session_audio_demand(
                        clip,
                        launch_frame,
                        performance_frame,
                        window.frames.min(fade),
                        visit,
                    )?;
                }
                next
            } else {
                route.owner
            };
            if let Some(release) = route
                .audio_release
                .filter(|release| release.active_at(performance_frame))
            {
                let remaining =
                    usize::try_from(release.end_frame() - performance_frame).unwrap_or(usize::MAX);
                self.visit_session_audio_demand(
                    release.clip,
                    release.launch_frame,
                    performance_frame,
                    window.frames.min(remaining),
                    visit,
                )?;
            }
            match owner {
                RouteOwner::Arrangement => {
                    let track = self
                        .revision
                        .arrangement()
                        .tracks()
                        .get(index)
                        .ok_or(Invalid("stream demand track missing"))?;
                    // Buffered/ordered paths read raw audio even when postmix is muted.
                    let buffered = raw_audio
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
                            &mut |extent| visit(extent.into()),
                        )?;
                    }
                }
                RouteOwner::SessionAudio { clip, launch_frame } => {
                    self.visit_session_audio_demand(
                        clip,
                        launch_frame,
                        performance_frame,
                        window.frames,
                        visit,
                    )?;
                }
                RouteOwner::Session { .. } | RouteOwner::Silence => {}
            }
        }
        for binding in self
            .revision
            .scalar_automation
            .as_deref()
            .unwrap_or_default()
        {
            let delay = binding.arrival_frames;
            if delay == 0 {
                crate::stream_demand::visit_scalar_page_range(
                    &binding.request.source,
                    window.arrangement_frame,
                    window.frames,
                    &mut *visit,
                )?;
                continue;
            }
            let (owner, lead) = match history {
                ScalarOriginHistory::Current(owner) => (
                    Some(owner),
                    performance_frame
                        .checked_sub(owner.performance_frame)
                        .ok_or(Invalid("scalar origin frontier is after demand"))?,
                ),
                ScalarOriginHistory::Empty => (None, 0),
            };
            crate::scalar_automation::visit_origin_runs(
                window.frames,
                |offset| {
                    let position = lead
                        .checked_add(offset as u64)
                        .ok_or(Invalid("scalar origin offset overflow"))?;
                    let history = owner
                        .and_then(|owner| owner.compensation.as_ref())
                        .map(|compensation| &compensation.delays);
                    crate::compensation::CompensationRuntime::preview_stage_frame(
                        history,
                        delay,
                        position,
                        |source| {
                            if source >= lead {
                                return window
                                    .arrangement_frame
                                    .checked_add(source - lead)
                                    .map(Some)
                                    .ok_or(Invalid("scalar origin frame overflow"));
                            }
                            let owner =
                                owner.ok_or(Invalid("scalar projection owner is unavailable"))?;
                            let projected = owner
                                .performance_frame
                                .checked_add(source)
                                .ok_or(Invalid("scalar origin performance overflow"))?;
                            Ok(Some(owner.project_stream_arrangement(projected)?.0))
                        },
                    )
                },
                |_, frame, len| {
                    crate::stream_demand::visit_scalar_page_range(
                        &binding.request.source,
                        frame,
                        len,
                        &mut *visit,
                    )
                },
            )?;
        }
        Ok(window)
    }

    /// Pages one streamed Session audio clip reads for `frames` frames from
    /// `performance_frame` on its launch clock. Resident and inactive clips
    /// demand nothing.
    fn visit_session_audio_demand(
        &self,
        clip: ClipId,
        launch_frame: u64,
        performance_frame: u64,
        frames: usize,
        visit: &mut impl FnMut(StreamResourceExtent) -> Result<(), Invalid>,
    ) -> Result<(), Invalid> {
        let clip = self
            .revision
            .session()
            .audio_clip(clip)
            .ok_or(Invalid("stream demand clip missing"))?;
        if !clip.active {
            return Ok(());
        }
        let PreparedAudioSource::Streamed(source) = clip.source() else {
            return Ok(());
        };
        let Some(elapsed) = performance_frame.checked_sub(launch_frame) else {
            return Ok(());
        };
        let Some(start) = clip.source_frame(elapsed) else {
            return Ok(());
        };
        let count = frames.min(usize::try_from(clip.playback_end() - start).unwrap_or(usize::MAX));
        crate::stream_demand::visit_page_range(source, start, count, &mut |extent| {
            visit(extent.into())
        })?;
        // Later complete cycles demand the same pages. One suffix and one
        // cycle prefix cover the union, independent of block length or cycle
        // density.
        if let Some(range) = clip.loop_range {
            let rest = (frames - count)
                .min(usize::try_from(range.end() - range.start()).unwrap_or(usize::MAX));
            if rest > 0 {
                crate::stream_demand::visit_page_range(
                    source,
                    range.start(),
                    rest,
                    &mut |extent| visit(extent.into()),
                )?;
            }
        }
        Ok(())
    }
}
