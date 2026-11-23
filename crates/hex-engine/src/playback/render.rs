//! Playback block rendering, paused monitors, signal observers, and streamed page paths.
use super::*;

impl Playback {
    pub fn render(&mut self, output: &mut [Frame]) -> Result<(), Invalid> {
        self.render_with_post_node_observer(output, None)
    }

    /// Render a separately prepared paused monitor without mutating the
    /// installed Arrangement or terminating an in-flight transport fade.
    pub fn render_with_paused_track_monitor(
        &self,
        monitor: &mut crate::session_playback::PausedTrackMonitor,
        input: &[Frame],
        output: &mut [Frame],
    ) -> Result<(), Invalid> {
        monitor
            .render(self.paused_monitor_runtime(output)?, input, output)
            .map_err(|error| Invalid(error.diagnostic_message()))
    }

    /// Render an independent keyboard audition over a settled paused graph.
    pub fn render_paused_keyboard_monitor(
        &self,
        monitor: &mut crate::PausedKeyboardMonitor,
        output: &mut [Frame],
    ) -> Result<(), Invalid> {
        monitor
            .render(self.paused_monitor_runtime(output)?, output)
            .map_err(|error| Invalid(error.diagnostic_message()))
    }

    /// Advance only the paused monitor's retained audio after input removal.
    /// `false` means this was the final block, which can still contain output.
    pub fn drain_paused_track_monitor(
        &self,
        monitor: &mut crate::session_playback::PausedTrackMonitor,
        output: &mut [Frame],
    ) -> Result<bool, Invalid> {
        monitor
            .drain(self.paused_monitor_runtime(output)?, output)
            .map_err(|error| Invalid(error.diagnostic_message()))
    }

    /// Whether installing this candidate must wait for the audible private
    /// PCM predecessor's bounded device transition to finish.
    pub fn paused_track_monitor_blocks_replacement(
        &self,
        monitor: &crate::PausedTrackMonitor,
    ) -> bool {
        self.session_runtime
            .as_ref()
            .is_some_and(|runtime| monitor.blocks_graph_replacement(runtime))
    }

    /// Render the paused graph through its separately admitted foreign effect
    /// endpoints. The supplied callback view must belong to the monitor and
    /// preserve the host's ordinary final-format acceptance frontier.
    pub fn render_paused_track_monitor_with_foreign(
        &self,
        monitor: &mut crate::PausedTrackMonitor,
        input: &[Frame],
        output: &mut [Frame],
        foreign: &mut dyn hex_lattice::ForeignProcessors,
    ) -> Result<(), Invalid> {
        monitor
            .render_with_foreign(self.paused_monitor_runtime(output)?, input, output, foreign)
            .map_err(|error| Invalid(error.diagnostic_message()))
    }

    /// Render loaned resident notes through the private downstream graph.
    /// The host must route instrument calls to the original worker, effect
    /// calls to the monitor's exact workers, and accept staging only after
    /// final output conversion. On error it retires the monitor on control.
    /// Resume, Stop, seek (even to the same frame), and source replacement
    /// end this activation and require fresh ownership before rendering again.
    pub fn render_paused_source_monitor_with_foreign(
        &mut self,
        monitor: &mut crate::PausedSourceMonitor,
        output: &mut [Frame],
        observer: Option<&mut crate::NodeSignalObserver<'_>>,
        foreign: &mut dyn hex_lattice::ForeignProcessors,
    ) -> Result<(), Invalid> {
        let settled = self.pending_transport_fade().is_none();
        let result = match self.session_runtime.as_mut() {
            Some(runtime) if settled => monitor
                .render(runtime, output, observer, foreign)
                .map_err(|error| Invalid(error.diagnostic_message())),
            _ => Err(Invalid(
                "paused source monitoring requires a settled session runtime",
            )),
        };
        if result.is_err() {
            foreign.reject_callback();
            output.fill([0.0; 2]);
        } else {
            self.reset_metronome();
            self.mix_tuning_audition(output);
        }
        result
    }

    /// Opt this resident graph into the output-owned paused source route.
    /// Ordinary pause blocks (including the suffix of a transport fade) hold
    /// the Arrangement until a private monitor can render the loaned notes.
    /// Call before publishing the graph on control.
    pub fn defer_hosted_paused_source_monitor_on_control(&mut self) -> bool {
        if self
            .prepared_revision
            .as_deref()
            .is_some_and(crate::PausedSourceMonitor::supports_revision_on_control)
            && let Some(runtime) = self.session_runtime.as_mut()
        {
            runtime.defer_paused_source_monitor = true;
            return true;
        }
        false
    }

    /// Continue the same monitor callback with zero input. The final block
    /// can contain audio even when the engine reports no remaining tail.
    pub fn drain_paused_track_monitor_with_foreign(
        &self,
        monitor: &mut crate::PausedTrackMonitor,
        output: &mut [Frame],
        foreign: &mut dyn hex_lattice::ForeignProcessors,
    ) -> Result<bool, Invalid> {
        monitor
            .drain_with_foreign(self.paused_monitor_runtime(output)?, output, foreign)
            .map_err(|error| Invalid(error.diagnostic_message()))
    }

    pub(super) fn paused_monitor_runtime(
        &self,
        output: &mut [Frame],
    ) -> Result<&SessionPlayback, Invalid> {
        if let Some(runtime) = self.session_runtime.as_ref()
            && self.pending_transport_fade().is_none()
        {
            return Ok(runtime);
        }
        output.fill([0.0; 2]);
        Err(Invalid(
            "paused track monitoring requires a settled session runtime",
        ))
    }

    /// Render one bounded physical callback with a borrowed live track block.
    /// Track input follows transport fades; only timing-only project taps are admitted.
    #[allow(clippy::too_many_arguments)]
    pub fn render_with_live_track_audio(
        &mut self,
        output: &mut [Frame],
        track: hex_project::TrackId,
        input: &[Frame],
        pages: Option<&mut dyn MediaPages>,
        observer: Option<&mut crate::NodeSignalObserver<'_>>,
        project: Option<&mut crate::ProjectOutputTap<'_, '_>>,
        foreign: &mut dyn hex_lattice::ForeignProcessors,
    ) -> Result<(), Invalid> {
        self.render_live_audio(
            output, track, input, pages, observer, project, None, foreign,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render_live_audio<P: MediaPages + ?Sized>(
        &mut self,
        output: &mut [Frame],
        track: hex_project::TrackId,
        input: &[Frame],
        mut pages: Option<&mut P>,
        mut observer: Option<&mut crate::NodeSignalObserver<'_>>,
        mut project: Option<&mut crate::ProjectOutputTap<'_, '_>>,
        sources: Option<&crate::native_device_resources::current_sampler::CurrentSamplerSources>,
        foreign: &mut dyn hex_lattice::ForeignProcessors,
    ) -> Result<(), Invalid> {
        let source_ref = sources.map(crate::native_device_resources::SamplerSourcesRef::Current);
        let valid = self.session_runtime.as_ref().is_some_and(|runtime| {
            runtime
                .validate_live_audio(
                    track,
                    input,
                    output.len(),
                    project.as_ref().is_some_and(|tap| !tap.is_timing_only()),
                    source_ref,
                )
                .is_ok()
        });
        if !valid {
            output.fill([0.0; 2]);
            foreign.reject_callback();
            return Err(Invalid("invalid live track audio request"));
        }
        self.render_declicked(output, |playback, output, offset| {
            let mut shifted = ShiftedForeign {
                inner: &mut *foreign,
                offset,
            };
            let result = playback
                .session_runtime
                .as_mut()
                .ok_or(Invalid(
                    "live track audio requires a prepared session runtime",
                ))
                .and_then(|runtime| {
                    with_part_offset(project.as_deref_mut(), offset, |project| {
                        with_shifted_observer(observer.as_deref_mut(), offset, |observer| {
                            if runtime.playing() {
                                runtime.render_live_audio(
                                    output,
                                    crate::session_playback::LiveAudioInputs {
                                        live_audio: crate::session_playback::LiveTrackAudio {
                                            track,
                                            frames: &input[offset..offset + output.len()],
                                        },
                                        sources: source_ref,
                                    },
                                    pages.as_deref_mut(),
                                    observer,
                                    project,
                                    &mut shifted,
                                )
                            } else if let Some(sources) = sources {
                                runtime.render_current_sampler_with_foreign(
                                    output,
                                    pages.as_deref_mut(),
                                    observer,
                                    project,
                                    sources,
                                    &mut shifted,
                                )
                            } else {
                                runtime.render_internal_without_live_audio(
                                    output,
                                    pages.as_deref_mut(),
                                    observer,
                                    project,
                                    &mut shifted,
                                )
                            }
                            .map_err(|error| Invalid(error.diagnostic_message()))
                        })
                    })
                });
            if result.is_err() {
                foreign.reject_callback();
                output.fill([0.0; 2]);
            }
            result
        })
    }

    /// The normal canonical renderer with the audio owner's already retained
    /// foreign endpoints. No finite fallback may omit a required native effect.
    /// Staged input is still uncommitted: only the audio owner can accept it
    /// after final host conversion. Monitor audio remains after the project tap.
    pub fn render_with_foreign_processors(
        &mut self,
        output: &mut [Frame],
        pages: Option<&mut dyn MediaPages>,
        observer: Option<&mut crate::NodeSignalObserver<'_>>,
        project: Option<&mut crate::ProjectOutputTap<'_, '_>>,
        foreign: &mut dyn hex_lattice::ForeignProcessors,
    ) -> Result<(), Invalid> {
        let mut pages = pages;
        let mut observer = observer;
        let mut project = project;
        self.render_declicked(output, |playback, output, offset| {
            let mut shifted = ShiftedForeign {
                inner: &mut *foreign,
                offset,
            };
            let result = playback
                .session_runtime
                .as_mut()
                .ok_or(Invalid(
                    "foreign effects require a prepared session runtime",
                ))
                .and_then(|runtime| {
                    with_part_offset(project.as_deref_mut(), offset, |project| {
                        with_shifted_observer(observer.as_deref_mut(), offset, |observer| {
                            runtime
                                .render_with_foreign_processors(
                                    output,
                                    pages
                                        .as_mut()
                                        .map(|pages| &mut **pages as &mut dyn MediaPages),
                                    observer,
                                    project,
                                    &mut shifted,
                                )
                                .map_err(|error| Invalid(error.diagnostic_message()))
                        })
                    })
                });
            if result.is_err() {
                foreign.reject_callback();
                output.fill([0.0; 2]);
            }
            result
        })
    }

    /// Render while exposing bounded borrowed output from each processed
    /// native device node. The observer is audio-thread code when used by
    /// `hex-audio`: it must not allocate, block or retain the slice.
    pub fn render_with_post_node_observer(
        &mut self,
        output: &mut [Frame],
        observer: Option<&mut crate::PostNodeObserver<'_>>,
    ) -> Result<(), Invalid> {
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
    ) -> Result<(), Invalid> {
        self.render_with_signal_observers(output, observer, None)
    }

    /// Observe the actual project mix before monitor-only metronome audio.
    /// A fallback without the canonical Session renderer reports Unavailable
    /// through the project tap and preserves its ordinary audio behavior.
    pub fn render_with_signal_observers(
        &mut self,
        output: &mut [Frame],
        observer: Option<&mut crate::NodeSignalObserver<'_>>,
        project: Option<&mut crate::ProjectOutputTap<'_, '_>>,
    ) -> Result<(), Invalid> {
        let mut observer = observer;
        let mut project = project;
        self.render_declicked(output, |playback, output, offset| {
            playback.render_signal_part(
                output,
                offset,
                observer.as_deref_mut(),
                project.as_deref_mut(),
            )
        })
    }

    pub(super) fn render_signal_part(
        &mut self,
        output: &mut [Frame],
        offset: usize,
        observer: Option<&mut crate::NodeSignalObserver<'_>>,
        mut project: Option<&mut crate::ProjectOutputTap<'_, '_>>,
    ) -> Result<(), Invalid> {
        if let Some(runtime) = self.session_runtime.as_mut() {
            return with_part_offset(project, offset, |project| {
                with_shifted_observer(observer, offset, |observer| {
                    runtime
                        .render_with_signal_observers(output, observer, project)
                        .map_err(|error| Invalid(error.diagnostic_message()))
                })
            });
        }
        if let Some(project) = project.as_mut() {
            project.unavailable();
        }
        if self.session_unavailable.is_some()
            && (self
                .prepared_revision
                .as_ref()
                .is_some_and(|revision| revision.session().has_mutable_instruments())
                || !self.project.main_device_chain.is_empty()
                || self
                    .project
                    .tracks()
                    .iter()
                    .any(|track| track.has_device_chain() || track.has_instrument_source()))
        {
            return Err(Invalid("prepared native playback runtime is unavailable"));
        }
        if self.playing && !output.is_empty() && self.contains_streamed_sources() {
            return Err(Invalid("streamed playback requires a media page provider"));
        }
        let (next_performance, clock_exhausted) = if self.playing {
            match self.performance_frame.checked_add(
                u64::try_from(output.len()).map_err(|_| Invalid("frame count overflow"))?,
            ) {
                Some(frame) if !self.performance_clock_exhausted => (frame, false),
                Some(_) | None => (u64::MAX, true),
            }
        } else {
            (self.performance_frame, self.performance_clock_exhausted)
        };
        self.render_finite_arrangement_with_pages::<dyn MediaPages>(output, None)?;
        self.performance_frame = next_performance;
        self.performance_clock_exhausted = clock_exhausted;
        Ok(())
    }

    pub(crate) fn session_runtime_mut_for_stream_action(
        &mut self,
    ) -> Result<&mut SessionPlayback, PlaybackActionError> {
        self.session_runtime
            .as_deref_mut()
            .ok_or(PlaybackActionError::SessionNotPrepared)
    }
    pub(crate) fn visit_next_stream_chunk(
        &self,
        maximum_frames: usize,
        playing: Option<bool>,
        visit: &mut impl FnMut(crate::StreamResourceExtent) -> Result<(), Invalid>,
    ) -> Result<crate::StreamChunkWindow, Invalid> {
        self.session_runtime
            .as_ref()
            .ok_or(Invalid("candidate session runtime unavailable"))?
            .visit_next_stream_chunk(maximum_frames, playing, visit)
    }
    pub(crate) fn visit_ordinary_stream_chunk(
        &self,
        maximum_frames: usize,
        playing: Option<bool>,
        visit: &mut impl FnMut(crate::StreamResourceExtent) -> Result<(), Invalid>,
    ) -> Result<crate::StreamChunkWindow, Invalid> {
        self.session_runtime
            .as_ref()
            .ok_or(Invalid("ordinary session runtime unavailable"))?
            .visit_ordinary_stream_chunk(maximum_frames, playing, visit)
    }
    pub(crate) fn visit_current_stream_quantum(
        &mut self,
        maximum_frames: usize,
        playing: Option<bool>,
        ordinary: bool,
        visit: &mut impl FnMut(crate::StreamResourceExtent) -> Result<(), Invalid>,
    ) -> Result<usize, Invalid> {
        self.session_runtime
            .as_mut()
            .ok_or(Invalid("session runtime unavailable"))?
            .visit_current_stream_quantum(maximum_frames, playing, ordinary, visit)
    }
    pub(crate) fn visit_current_stream_quantum_at(
        &mut self,
        start_offset: u64,
        maximum_frames: usize,
        playing: Option<bool>,
        ordinary: bool,
        visit: &mut impl FnMut(crate::StreamResourceExtent) -> Result<(), Invalid>,
    ) -> Result<usize, crate::playback::StreamQuantumProjectionError> {
        self.session_runtime
            .as_mut()
            .ok_or(crate::playback::StreamQuantumProjectionError::Projection(
                Invalid("session runtime unavailable"),
            ))?
            .visit_current_stream_quantum_at(start_offset, maximum_frames, playing, ordinary, visit)
    }
    pub(crate) fn preview_stream_handoff_with_report(
        &mut self,
        previous: &mut Self,
        maximum_frames: usize,
        playing: Option<bool>,
        visit: &mut impl FnMut(crate::StreamResourceExtent) -> Result<(), Invalid>,
        ordinary: bool,
        fenced_restart: bool,
    ) -> Result<(crate::StreamChunkWindow, crate::SessionHandoffReport), PlaybackHandoffError> {
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
    #[allow(clippy::too_many_arguments)] // Exact borrowed comparison plus the existing preview inputs.
    pub(crate) fn preview_stream_handoff_with_continuation_and_report(
        &mut self,
        previous: &mut Self,
        maximum_frames: usize,
        playing: Option<bool>,
        visit: &mut impl FnMut(crate::StreamResourceExtent) -> Result<(), Invalid>,
        ordinary: bool,
        fenced_restart: bool,
        continuation: Option<&hex_lattice::ForeignConfigurationContinuation<'_>>,
    ) -> Result<(crate::StreamChunkWindow, crate::SessionHandoffReport), PlaybackHandoffError> {
        if fenced_restart && !self.live_restart_requested() {
            return Err(PlaybackHandoffError::RuntimeFenceChanged);
        }
        let invalid = || PlaybackHandoffError::Session(crate::SessionPlaybackError::InvalidCommand);
        let next = self.session_runtime.as_mut().ok_or_else(invalid)?;
        let old = previous.session_runtime.as_mut().ok_or_else(invalid)?;
        next.preview_stream_handoff_with_continuation_and_report(
            old,
            maximum_frames,
            playing,
            visit,
            ordinary,
            fenced_restart,
            continuation,
        )
        .map_err(PlaybackHandoffError::Session)
    }
    pub(crate) fn forecast_tempo_cut(
        &self,
        previous: &Self,
        maximum_frames: usize,
        visit: &mut impl FnMut(crate::StreamResourceExtent) -> Result<(), Invalid>,
        anchor: &crate::session_playback::PreparedTempoAnchor,
        claimed: Option<crate::session_playback::ClaimedTempoPair>,
    ) -> Result<crate::StreamChunkWindow, PlaybackHandoffError> {
        let invalid = || PlaybackHandoffError::Session(crate::SessionPlaybackError::InvalidCommand);
        let next = self.session_runtime.as_ref().ok_or_else(invalid)?;
        let old = previous.session_runtime.as_ref().ok_or_else(invalid)?;
        next.forecast_tempo_cut(old, maximum_frames, visit, anchor, claimed)
            .map_err(PlaybackHandoffError::Session)
    }

    pub(crate) fn preview_tempo_cut(
        &mut self,
        previous: &mut Self,
        maximum_frames: usize,
        visit: &mut impl FnMut(crate::StreamResourceExtent) -> Result<(), Invalid>,
        anchor: &crate::session_playback::PreparedTempoAnchor,
        claimed: Option<crate::session_playback::ClaimedTempoPair>,
    ) -> Result<crate::StreamChunkWindow, PlaybackHandoffError> {
        let invalid = || PlaybackHandoffError::Session(crate::SessionPlaybackError::InvalidCommand);
        let next = self.session_runtime.as_mut().ok_or_else(invalid)?;
        let old = previous.session_runtime.as_mut().ok_or_else(invalid)?;
        next.preview_tempo_cut(old, maximum_frames, visit, anchor, claimed)
            .map_err(PlaybackHandoffError::Session)
    }

    pub(crate) fn preview_stream_handoff_with_stored_foreign_continuation(
        &mut self,
        previous: &mut Self,
        maximum_frames: usize,
        playing: Option<bool>,
        visit: &mut impl FnMut(crate::StreamResourceExtent) -> Result<(), Invalid>,
    ) -> Result<crate::StreamChunkWindow, PlaybackHandoffError> {
        let invalid = || PlaybackHandoffError::Session(crate::SessionPlaybackError::InvalidCommand);
        let next = self.session_runtime.as_mut().ok_or_else(invalid)?;
        let old = previous.session_runtime.as_mut().ok_or_else(invalid)?;
        let Some(states) = next.foreign_continuation.take() else {
            return Err(PlaybackHandoffError::RuntimeFenceChanged);
        };
        let result = match states.matcher() {
            Ok(matcher) => next
                .preview_stream_handoff_with_continuation_and_report(
                    old,
                    maximum_frames,
                    playing,
                    visit,
                    true,
                    false,
                    Some(&matcher),
                )
                .map(|(window, _)| window)
                .map_err(PlaybackHandoffError::Session),
            Err(_) => Err(PlaybackHandoffError::RuntimeFenceChanged),
        };
        next.foreign_continuation = Some(states);
        result
    }

    /// Render through the same Arrangement loop segmentation or mixed Session
    /// route kernel while borrowing streamed pages from the caller.
    pub fn render_with_pages<P: MediaPages>(
        &mut self,
        output: &mut [Frame],
        pages: &mut P,
    ) -> Result<(), Invalid> {
        self.render_declicked(output, |playback, output, _| {
            if let Some(runtime) = playback.session_runtime.as_mut() {
                return runtime
                    .render_with_pages(output, pages)
                    .map_err(|error| Invalid(error.diagnostic_message()));
            }
            playback.render_pages_fallback(output, pages)
        })
    }

    /// Borrow pages and optional post-node observation for one block. Neither
    /// borrow is retained; the observer must not allocate, block, clone owners
    /// or retain samples. Observations commit only after successful block and
    /// final output validation. Observed rendering requires a Session runtime;
    /// refusal leaves output, clocks, page provider and playback state unchanged.
    /// With no observer, the existing finite page-rendering fallback is retained.
    pub fn render_with_pages_and_post_node_observer<P: MediaPages + ?Sized>(
        &mut self,
        output: &mut [Frame],
        pages: &mut P,
        observer: Option<&mut crate::PostNodeObserver<'_>>,
    ) -> Result<(), Invalid> {
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
    ) -> Result<(), Invalid> {
        self.render_with_pages_and_signal_observers(output, pages, observer, None)
    }

    /// Page-backed project/device observations from one actual render pass.
    pub fn render_with_pages_and_signal_observers<P: MediaPages + ?Sized>(
        &mut self,
        output: &mut [Frame],
        pages: &mut P,
        observer: Option<&mut crate::NodeSignalObserver<'_>>,
        project: Option<&mut crate::ProjectOutputTap<'_, '_>>,
    ) -> Result<(), Invalid> {
        let mut observer = observer;
        let mut project = project;
        self.render_declicked(output, |playback, output, offset| {
            let Some(runtime) = playback.session_runtime.as_mut() else {
                if let Some(project) = project.as_deref_mut() {
                    project.unavailable();
                }
                if observer.is_some() {
                    return Err(Invalid(
                        "post-node observation requires a prepared session runtime",
                    ));
                }
                return playback.render_pages_fallback(output, pages);
            };
            with_part_offset(project.as_deref_mut(), offset, |project| {
                with_shifted_observer(observer.as_deref_mut(), offset, |observer| {
                    runtime
                        .render_with_pages_and_signal_observers(output, pages, observer, project)
                        .map_err(|error| Invalid(error.diagnostic_message()))
                })
            })
        })
    }

    /// Bounded current canonical identity check, intended once per selection or
    /// active revision change rather than once per captured sample.
    pub fn project_output_supported(&self, tracks: &[u64]) -> bool {
        self.session_runtime.is_some() && self.project.project_track_ids.contains(tracks, || {})
    }

    /// Check selected sources against the current canonical render graph.
    pub fn project_sample_sources_supported(
        &self,
        tracks: &[u64],
        returns: &[crate::ReturnId],
    ) -> bool {
        self.project_output_supported(tracks)
            && returns.len() <= crate::MAX_RETURN_TRACKS
            && returns
                .iter()
                .all(|id| self.project.return_buses().iter().any(|bus| bus.id == *id))
    }

    pub(super) fn render_pages_fallback<P: MediaPages + ?Sized>(
        &mut self,
        output: &mut [Frame],
        pages: &mut P,
    ) -> Result<(), Invalid> {
        if self.session_unavailable.is_some()
            && (self
                .prepared_revision
                .as_ref()
                .is_some_and(|revision| revision.session().has_mutable_instruments())
                || !self.project.main_device_chain.is_empty()
                || self
                    .project
                    .tracks()
                    .iter()
                    .any(|track| track.has_device_chain() || track.has_instrument_source()))
        {
            return Err(Invalid("prepared native playback runtime is unavailable"));
        }
        let (next_performance, clock_exhausted) = if self.playing {
            match self.performance_frame.checked_add(
                u64::try_from(output.len()).map_err(|_| Invalid("frame count overflow"))?,
            ) {
                Some(frame) if !self.performance_clock_exhausted => (frame, false),
                Some(_) | None => (u64::MAX, true),
            }
        } else {
            (self.performance_frame, self.performance_clock_exhausted)
        };
        self.render_finite_arrangement_with_pages(output, Some(pages))?;
        self.performance_frame = next_performance;
        self.performance_clock_exhausted = clock_exhausted;
        Ok(())
    }

    /// Render one callback as parts split at a pending fade end.
    /// Each part is followed by its declick gain and then monitor-only metronome
    /// and tuning audio. A refused first part keeps the renderer's own atomic
    /// refusal (the output is left as the renderer left it). A part refused
    /// after an earlier part rendered silences the whole callback; rendered
    /// parts keep their committed clock, as consecutive callbacks would.
    /// Without a pending fade end this is exactly one part.
    pub(super) fn render_declicked(
        &mut self,
        output: &mut [Frame],
        mut part: impl FnMut(&mut Self, &mut [Frame], usize) -> Result<(), Invalid>,
    ) -> Result<(), Invalid> {
        let frames = transport_fade::fade_frames(self.sample_rate());
        let mut offset = 0;
        loop {
            if self.transport_fade.device_insert_held() {
                output[offset..].fill([0.0; 2]);
                return Ok(());
            }
            let start = self.position();
            let playing = self.playing();
            let part_len = self.transport_fade.part_len(output.len() - offset, frames);
            let end = offset + part_len;
            let chunk = &mut output[offset..end];
            if let Err(error) = part(self, chunk, offset) {
                if offset > 0 {
                    output.fill([0.0; 2]);
                }
                return Err(error);
            }
            let completed = self.transport_fade.apply(chunk, frames);
            self.mix_metronome(start, playing, chunk);
            self.mix_tuning_audition(chunk);
            if completed {
                self.complete_transport_fade();
            }
            if self.transport_fade.apply_device_insert_cut(chunk, frames)
                && let Some(runtime) = self.session_runtime.as_deref_mut()
            {
                runtime.set_playing_at_device_insert_cut(false);
            }
            offset = end;
            if offset >= output.len() {
                return Ok(());
            }
        }
    }

    pub(super) fn render_finite_arrangement_with_pages<P: MediaPages + ?Sized>(
        &mut self,
        output: &mut [Frame],
        mut pages: Option<&mut P>,
    ) -> Result<(), Invalid> {
        let Some(region) = self
            .project
            .loop_region()
            .filter(|region| region.enabled() && self.position < region.end())
        else {
            return self.render_linear_with_pages(output, pages);
        };
        output.fill([0.0; 2]);
        if !self.playing || output.is_empty() {
            return Ok(());
        }

        let start = region.start();
        let end = region.end();
        let length = end - start;
        let mut output_offset = 0_usize;
        while output_offset < output.len() {
            if self.position == end {
                self.position = start;
            }
            let remaining = output.len() - output_offset;
            if self.position == start
                && let Ok(period_length) = usize::try_from(length)
            {
                let stateful = self
                    .linear_runtime
                    .as_ref()
                    .is_some_and(|runtime| runtime.has_stateful_processors());
                if period_length > 0 && period_length <= remaining && !stateful {
                    let period_end = output_offset + period_length;
                    self.render_linear_with_pages(
                        &mut output[output_offset..period_end],
                        pages.as_deref_mut(),
                    )?;
                    self.position = start;

                    let mut filled = period_length;
                    while filled < remaining {
                        let copy_length = filled.min(remaining - filled);
                        let source_start = output_offset;
                        let source_end = source_start + copy_length;
                        let destination = output_offset + filled;
                        output.copy_within(source_start..source_end, destination);
                        filled += copy_length;
                    }
                    let offset = u64::try_from(remaining % period_length)
                        .map_err(|_| Invalid("loop offset overflow"))?;
                    self.position = start + offset;
                    output_offset = output.len();
                    continue;
                }
            }
            let span_length = if self.position < start {
                remaining.min(usize::try_from(start - self.position).unwrap_or(usize::MAX))
            } else {
                remaining.min(usize::try_from(end - self.position).unwrap_or(usize::MAX))
            };
            if span_length == 0 {
                self.position = start;
                continue;
            }
            let span_end = output_offset + span_length;
            self.render_linear_with_pages(
                &mut output[output_offset..span_end],
                pages.as_deref_mut(),
            )?;
            output_offset = span_end;
            if self.position == end {
                self.position = start;
            }
        }
        Ok(())
    }

    pub(super) fn render_linear_with_pages<P: MediaPages + ?Sized>(
        &mut self,
        output: &mut [Frame],
        pages: Option<&mut P>,
    ) -> Result<(), Invalid> {
        let runtime = self
            .linear_runtime
            .as_mut()
            .ok_or(Invalid("linear renderer runtime is unavailable"))?;
        if let Some(pages) = pages {
            runtime.render_with_pages(
                &self.project,
                &mut self.position,
                self.playing,
                output,
                Some(pages),
            )
        } else {
            runtime.render_without_pages(&self.project, &mut self.position, self.playing, output)
        }
    }
}

/// Report a render part's node signals at their callback-relative offset.
pub(super) fn with_shifted_observer<R>(
    observer: Option<&mut crate::NodeSignalObserver<'_>>,
    offset: usize,
    render: impl FnOnce(Option<&mut crate::NodeSignalObserver<'_>>) -> R,
) -> R {
    match observer {
        Some(inner) if offset != 0 => {
            let mut shifted = |identity: crate::PostNodeIdentity,
                               phase: crate::NodeSignalPhase,
                               at: usize,
                               frames: &[Frame]| {
                inner(identity, phase, offset.saturating_add(at), frames);
            };
            render(Some(&mut shifted))
        }
        other => render(other),
    }
}

/// Report a render part's project signals at their callback-relative offset.
pub(super) fn with_part_offset<'a, 'observer, R>(
    project: Option<&mut crate::ProjectOutputTap<'a, 'observer>>,
    offset: usize,
    render: impl FnOnce(Option<&mut crate::ProjectOutputTap<'a, 'observer>>) -> R,
) -> R {
    match project {
        Some(tap) => {
            let previous = tap.part_offset();
            tap.replace_part_offset(previous.saturating_add(offset));
            let result = render(Some(&mut *tap));
            tap.replace_part_offset(previous);
            result
        }
        None => render(None),
    }
}

/// Foreign endpoints stage one physical callback; a declicked render part
/// reports its segments at their callback-relative offset.
pub(super) struct ShiftedForeign<'a> {
    pub(super) inner: &'a mut dyn hex_lattice::ForeignProcessors,
    pub(super) offset: usize,
}

impl hex_lattice::ForeignProcessors for ShiftedForeign<'_> {
    fn returns_post_trim(&self, node: hex_lattice::ForeignProcessorNode<'_>) -> bool {
        self.inner.returns_post_trim(node)
    }
    fn process_segment(
        &mut self,
        node: hex_lattice::ForeignProcessorNode<'_>,
        frame_offset: usize,
        frames: &mut [Frame],
        clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
    ) -> Result<(), hex_lattice_contract::DeviceContractError> {
        self.inner.process_segment(
            node,
            self.offset.saturating_add(frame_offset),
            frames,
            clock,
        )
    }
    fn process_segment_with_parameters(
        &mut self,
        node: hex_lattice::ForeignProcessorNode<'_>,
        frame_offset: usize,
        frames: &mut [Frame],
        parameters: &[hex_lattice_contract::ForeignParameterPoint],
        clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
    ) -> Result<(), hex_lattice_contract::DeviceContractError> {
        self.process_segment_with_notes(node, frame_offset, frames, parameters, &[], clock)
    }
    fn process_segment_with_notes(
        &mut self,
        node: hex_lattice::ForeignProcessorNode<'_>,
        frame_offset: usize,
        frames: &mut [Frame],
        parameters: &[hex_lattice_contract::ForeignParameterPoint],
        notes: &[hex_lattice::ForeignNote],
        clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
    ) -> Result<(), hex_lattice_contract::DeviceContractError> {
        self.inner.process_segment_with_notes(
            node,
            self.offset.saturating_add(frame_offset),
            frames,
            parameters,
            notes,
            clock,
        )
    }
    fn process_segment_with_modulation(
        &mut self,
        node: hex_lattice::ForeignProcessorNode<'_>,
        frame_offset: usize,
        frames: &mut [Frame],
        lane: hex_lattice::ForeignModulationSamples<'_>,
        clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
    ) -> Result<(), hex_lattice_contract::DeviceContractError> {
        let Some(offset) = self.offset.checked_add(frame_offset) else {
            self.inner.reject_callback();
            frames.fill([0.0; 2]);
            return Err(hex_lattice_contract::DeviceContractError::ForeignProcessingFailed);
        };
        self.inner
            .process_segment_with_modulation(node, offset, frames, lane, clock)
    }
    fn committed_tail_is_drained(&self, node: hex_lattice::ForeignProcessorNode<'_>) -> bool {
        self.inner.committed_tail_is_drained(node)
    }
    fn reject_callback(&mut self) {
        self.inner.reject_callback();
    }
}

/// The legacy finite cursor at the donor cursor's Arrangement beat when the
/// tempo map changed (Pulse #4118, #4135); an unchanged map keeps the frame.
/// Every legacy cursor handoff (linear, promotion, demotion and transport
/// copy) uses it. Frame zero remains beat zero even without timing metadata. A cursor
/// inside the donor's active loop that remaps at or after the candidate loop
/// end wraps by whole loop lengths, as the finite loop renderer does. Linear
/// MIDI renders statelessly from the cursor, so a held note continues as the
/// candidate's occurrence at that beat and ends at its remapped gate.
pub(super) fn remapped_linear_position(
    next: &Project,
    previous: &Project,
    position: u64,
) -> Result<u64, PlaybackHandoffError> {
    use crate::MidiTiming as _;
    let conversion = || PlaybackHandoffError::Session(crate::SessionPlaybackError::TempoConversion);
    if position == 0 || next.tempo_map == previous.tempo_map {
        return Ok(position);
    }
    let (Some(new_timing), Some(old_timing)) =
        (next.midi_volume_timing(), previous.midi_volume_timing())
    else {
        return Err(conversion());
    };
    let beat = old_timing
        .frame_to_beat(position)
        .map_err(|_| conversion())?;
    let frame = new_timing.beat_to_frame(beat).map_err(|_| conversion())?;
    let looped = previous
        .loop_region()
        .is_some_and(|region| region.enabled() && position < region.end());
    let Some(region) = next
        .loop_region()
        .filter(|region| looped && region.enabled() && frame >= region.end())
    else {
        return Ok(frame);
    };
    let length = region
        .end()
        .checked_sub(region.start())
        .filter(|length| *length > 0)
        .ok_or_else(conversion)?;
    let offset = frame.checked_sub(region.start()).ok_or_else(conversion)?;
    Ok(region.start() + offset % length)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playback_render_outputs_silence_for_empty_project_and_advances_when_playing() {
        let project = Project::new(48_000, Vec::new()).unwrap();
        let mut playback = Playback::new(project);
        playback
            .try_apply_action(PlaybackAction::Transport(PlaybackCommand::PlayFrom(64)))
            .unwrap();
        let mut out = [[1.0; 2]; 16];
        playback.render(&mut out).unwrap();
        assert_eq!(out, [[0.0; 2]; 16]);
        assert_eq!(playback.position(), 80);
    }
}
