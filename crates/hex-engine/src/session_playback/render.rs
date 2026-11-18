mod compensated;
mod computer_keyboard;
mod instrument_automation;
mod shared_source;
mod source_live;
use super::capture::CaptureSink;
use super::{commands::transition_captures, *};
use crate::compile::PreparedAudioSource;
use crate::{MediaPages, MidiTiming, StreamedPageSpan, render_streamed_page_span};
use hex_project::LoopRegion;
use shared_source::{render_source_instrument_chunk, shared_source_has_route};
use std::sync::Arc;

#[derive(Clone, Copy)]
struct LiveTrackAudio<'a> {
    track: TrackId,
    frames: &'a [Frame],
}

impl SessionPlayback {
    /// An edit at the stopped origin may have retained an original bus/Main
    /// endpoint. Drain its actual zero-input tail before it can return. A
    /// paused transport away from the origin retains its existing freeze;
    /// explicit Stop has already reset the tail bank in the command owner.
    fn stopped_edit_tails(&self) -> bool {
        self.performance_frame == 0
            && self.arrangement_frame == 0
            && self
                .return_effect_tails
                .slots()
                .iter()
                .flatten()
                .any(|tail| tail.output_active)
    }

    /// Whether `render` skips every device chain: the transport is not
    /// playing and no source monitoring, keyboard, sampler console or
    /// stopped-edit tail or private PCM input history feeds them. The output
    /// is then silent and committed chain state stays frozen, so live device
    /// transitions cannot drain by rendering (Pulse #4558).
    pub(super) fn renders_no_device_chains(&self) -> bool {
        !self.playing
            && !self.monitor_source_releases
            && !self.stopped_input_has_history()
            && self.computer_keyboard.is_none()
            && self.current_sampler_keyboards.is_empty()
            && !self.stopped_edit_tails()
            && !self.foreign_arrangement.has_started()
            && !self.foreign_arrangement.has_keyboard()
    }

    pub(super) fn stopped_input_has_history(&self) -> bool {
        self.stopped_input.is_some_and(|input| input.rendered)
    }

    /// At Pause, an idle owned graph can leave its latency buffers for the
    /// next Play. Fixed buffering can hold upcoming Arrangement PCM even
    /// where the processor declares no residual tail. Keep the existing
    /// monitor path for live input, releases, residual tails and foreign
    /// sources. The stopped origin retains its existing handoff policy.
    pub(super) fn can_freeze_idle_latency_history(&self) -> bool {
        (self.performance_frame != 0 || self.arrangement_frame != 0)
            && !self.stopped_input_has_history()
            && self.revision.arrangement().main_latency_frames() != 0
            && self.computer_keyboard.is_none()
            && self.current_sampler_keyboards.is_empty()
            && self.foreign_arrangement.is_empty()
            && !self.has_analytic_release_output()
            && !self.source_effects.has_monitor_tail()
            && self
                .mutable_instruments
                .iter()
                .flatten()
                .all(|runtime| runtime.committed_is_quiescent())
            && self.tail_groups.iter().all(Option::is_none)
            && self.tail_mutable_instruments.iter().all(Option::is_none)
            && self
                .return_effect_tails
                .slots()
                .iter()
                .flatten()
                .all(|tail| !tail.output_active)
            && self
                .device_chains
                .iter()
                .chain(&self.return_chains)
                .chain(std::iter::once(&self.main_chain))
                .all(|chain| {
                    let range = 0..chain.len();
                    chain.foreign_bindings().next().is_none()
                        && (chain.output_tail_for_range(range.clone())
                            == Some(hex_lattice::OutputTail::Finite { max_tail_frames: 0 })
                            || chain.committed_range_is_quiescent(range))
                })
    }

    /// Render a complete block transactionally. A refusal after any chunk
    /// silences the requested block and leaves committed route/clock state as-is.
    pub fn render(&mut self, output: &mut [Frame]) -> Result<(), SessionPlaybackError> {
        self.render_with_post_node_observer(output, None)
    }

    /// Test-only comparison with the project tap for ordered resident track
    /// windows. Callers pass every canonical track in prepared order, with
    /// preallocated output for one render window. No worker or history moves.
    #[cfg(feature = "instrument-test-support")]
    #[doc(hidden)]
    pub fn render_prepared_track_windows_probe<const N: usize>(
        &mut self,
        ids: &[u64],
        tracks: &mut [[Frame; N]],
        output: &mut [Frame; N],
    ) -> Result<(), SessionPlaybackError> {
        let layout = self.revision.arrangement().tracks();
        if N == 0
            || N > 256
            || tracks.len() != ids.len()
            || ids.len() != layout.len()
            || layout
                .iter()
                .zip(ids)
                .any(|(track, id)| track.track_id.map(|canonical| canonical.value()) != Some(*id))
        {
            return Err(SessionPlaybackError::DeviceRuntime);
        }
        tracks.fill([[0.0; 2]; N]);
        let mut capture = super::capture::window_probe::TrackWindowProbe { ids, tracks };
        self.render_internal::<dyn MediaPages>(output, None, None, Some(&mut capture), None)
    }

    /// Mix borrowed live input into one track before its device chain while
    /// playing. The input and output lengths must match and may not exceed
    /// `MAX_OUTPUT_CAPTURE_FRAMES`. PCM project taps are refused; a timing-only
    /// tap is admitted. This public entry has no sampler resource scope.
    #[allow(clippy::too_many_arguments)]
    pub fn render_with_live_track_audio(
        &mut self,
        output: &mut [Frame],
        track: TrackId,
        input: &[Frame],
        pages: Option<&mut dyn MediaPages>,
        observer: Option<&mut crate::NodeSignalObserver<'_>>,
        project: Option<&mut ProjectOutputTap<'_, '_>>,
        foreign: &mut dyn hex_lattice::ForeignProcessors,
    ) -> Result<(), SessionPlaybackError> {
        self.render_live_audio(
            output, track, input, pages, observer, project, None, foreign,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render_live_audio<P: MediaPages + ?Sized>(
        &mut self,
        output: &mut [Frame],
        track: TrackId,
        input: &[Frame],
        pages: Option<&mut P>,
        observer: Option<&mut crate::NodeSignalObserver<'_>>,
        mut project: Option<&mut ProjectOutputTap<'_, '_>>,
        sources: Option<crate::native_device_resources::SamplerSourcesRef<'_>>,
        foreign: &mut dyn hex_lattice::ForeignProcessors,
    ) -> Result<(), SessionPlaybackError> {
        if self
            .validate_live_audio(
                track,
                input,
                output.len(),
                project.as_ref().is_some_and(|tap| !tap.is_timing_only()),
                sources,
            )
            .is_err()
        {
            output.fill([0.0; 2]);
            foreign.reject_callback();
            return Err(SessionPlaybackError::InvalidCommand);
        }
        self.candidate_admission_open = false;
        let capture = project
            .as_mut()
            .map(|tap| &mut **tap as &mut dyn CaptureSink);
        self.render_internal_with_live_audio(
            output,
            pages,
            observer,
            capture,
            sources,
            foreign,
            Some(LiveTrackAudio {
                track,
                frames: input,
            }),
        )
    }

    pub(crate) fn validate_live_audio(
        &self,
        track: TrackId,
        input: &[Frame],
        output_len: usize,
        has_project_tap: bool,
        sources: Option<crate::native_device_resources::SamplerSourcesRef<'_>>,
    ) -> Result<(), SessionPlaybackError> {
        if !self.playing
            || has_project_tap
            || output_len != input.len()
            || input.len() > MAX_OUTPUT_CAPTURE_FRAMES
            || !self.routes.iter().any(|route| route.id == track)
            || input.iter().flatten().any(|sample| !sample.is_finite())
            || (self.revision.resource_scope.is_some()
                && self.revision.resource_scope != sources.map(|source| source.id()))
        {
            return Err(SessionPlaybackError::InvalidCommand);
        }
        Ok(())
    }

    /// Only the separate paused input owner calls this after binding and
    /// validating the borrowed quantum. The ordinary live track entry stays
    /// restricted to the playing Arrangement runtime.
    pub(super) fn render_paused_pcm(
        &mut self,
        track: TrackId,
        input: &[Frame],
        output: &mut [Frame],
        foreign: &mut dyn hex_lattice::ForeignProcessors,
    ) -> Result<(), SessionPlaybackError> {
        if self.playing || !self.stopped_input.is_some_and(|scope| scope.track == track) {
            output.fill([0.0; 2]);
            return Err(SessionPlaybackError::InvalidCommand);
        }
        self.render_internal_with_live_audio::<dyn MediaPages>(
            output,
            None,
            None,
            None,
            None,
            foreign,
            Some(LiveTrackAudio {
                track,
                frames: input,
            }),
        )?;
        if !input.is_empty()
            && let Some(scope) = &mut self.stopped_input
        {
            scope.rendered = true;
        }
        Ok(())
    }

    /// Render using borrowed, already admitted foreign endpoints. This stages
    /// copied input only: the audio owner must separately accept the complete
    /// physical callback after final host conversion. Every refusal revokes
    /// staged foreign work; native state is never rolled back or replayed.
    pub fn render_with_foreign_processors(
        &mut self,
        output: &mut [Frame],
        pages: Option<&mut dyn MediaPages>,
        observer: Option<&mut crate::NodeSignalObserver<'_>>,
        mut project: Option<&mut ProjectOutputTap<'_, '_>>,
        foreign: &mut dyn hex_lattice::ForeignProcessors,
    ) -> Result<(), SessionPlaybackError> {
        self.candidate_admission_open = false;
        let capture = project
            .as_mut()
            .map(|tap| &mut **tap as &mut dyn CaptureSink);
        self.render_internal_with_foreign(output, pages, observer, capture, None, foreign)?;
        if let Some(project) = project {
            project.main(output);
        }
        Ok(())
    }

    pub fn render_control_sampler_export(
        &mut self,
        output: &mut [Frame],
        sources: &crate::native_device_resources::ControlSamplerSources,
    ) -> Result<(), SessionPlaybackError> {
        self.render_internal::<dyn MediaPages>(
            output,
            None,
            None,
            None,
            Some(crate::native_device_resources::SamplerSourcesRef::Control(
                sources,
            )),
        )
    }

    pub(crate) fn render_current_sampler_with_foreign<P: MediaPages + ?Sized>(
        &mut self,
        output: &mut [Frame],
        pages: Option<&mut P>,
        observer: Option<&mut crate::NodeSignalObserver<'_>>,
        mut project: Option<&mut ProjectOutputTap<'_, '_>>,
        sources: &crate::native_device_resources::current_sampler::CurrentSamplerSources,
        foreign: &mut dyn hex_lattice::ForeignProcessors,
    ) -> Result<(), SessionPlaybackError> {
        self.candidate_admission_open = false;
        let capture = project
            .as_mut()
            .map(|tap| &mut **tap as &mut dyn CaptureSink);
        self.render_internal_with_foreign(
            output,
            pages,
            observer,
            capture,
            Some(crate::native_device_resources::SamplerSourcesRef::Current(
                sources,
            )),
            foreign,
        )?;
        if let Some(project) = project {
            project.main(output);
        }
        Ok(())
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
        self.render_with_signal_observers(output, observer, None)
    }

    /// Observe native entries and selected direct track/Main signals in one
    /// transactional pass. Neither observer is a hardware acceptance receipt.
    pub fn render_with_signal_observers(
        &mut self,
        output: &mut [Frame],
        observer: Option<&mut crate::NodeSignalObserver<'_>>,
        mut project: Option<&mut ProjectOutputTap<'_, '_>>,
    ) -> Result<(), SessionPlaybackError> {
        self.candidate_admission_open = false;
        let contains_streamed_sources = if self.linear_arrangement_export {
            self.revision.arrangement().contains_streamed_sources()
        } else {
            self.revision.contains_streamed_sources()
        };
        if self.playing && !output.is_empty() && contains_streamed_sources {
            self.fail_midi_capture_render();
            return Err(SessionPlaybackError::MediaPagesUnavailable);
        }
        let capture = project
            .as_mut()
            .map(|tap| &mut **tap as &mut dyn CaptureSink);
        self.render_internal::<dyn MediaPages>(output, None, observer, capture, None)?;
        if let Some(project) = project {
            project.main(output);
        }
        Ok(())
    }

    /// Render with a borrowed page view. Misses silence only the unavailable
    /// source span; other tracks and the performance clock continue.
    pub fn render_with_pages<P: MediaPages>(
        &mut self,
        output: &mut [Frame],
        pages: &mut P,
    ) -> Result<(), SessionPlaybackError> {
        self.candidate_admission_open = false;
        self.render_internal(output, Some(pages), None, None, None)
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
        self.render_internal(output, pages, None, Some(capture), None)
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
        self.render_with_pages_and_signal_observers(output, pages, observer, None)
    }

    /// Borrowed-page counterpart with the same live output tap and scheduler.
    pub fn render_with_pages_and_signal_observers<P: MediaPages + ?Sized>(
        &mut self,
        output: &mut [Frame],
        pages: &mut P,
        observer: Option<&mut crate::NodeSignalObserver<'_>>,
        mut project: Option<&mut ProjectOutputTap<'_, '_>>,
    ) -> Result<(), SessionPlaybackError> {
        self.candidate_admission_open = false;
        let capture = project
            .as_mut()
            .map(|tap| &mut **tap as &mut dyn CaptureSink);
        self.render_internal(output, Some(pages), observer, capture, None)?;
        if let Some(project) = project {
            project.main(output);
        }
        Ok(())
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
        capture: Option<&mut dyn CaptureSink>,
        sources: Option<crate::native_device_resources::SamplerSourcesRef<'_>>,
    ) -> Result<(), SessionPlaybackError> {
        self.render_internal_with_foreign(
            output,
            pages,
            observer,
            capture,
            sources,
            &mut hex_lattice::RefuseForeign,
        )
    }

    pub(crate) fn render_internal_without_live_audio<P: MediaPages + ?Sized>(
        &mut self,
        output: &mut [Frame],
        pages: Option<&mut P>,
        observer: Option<&mut crate::NodeSignalObserver<'_>>,
        mut project: Option<&mut ProjectOutputTap<'_, '_>>,
        foreign: &mut dyn hex_lattice::ForeignProcessors,
    ) -> Result<(), SessionPlaybackError> {
        let capture = project
            .as_mut()
            .map(|tap| &mut **tap as &mut dyn CaptureSink);
        self.render_internal_with_foreign(output, pages, observer, capture, None, foreign)
    }

    fn render_internal_with_foreign<P: MediaPages + ?Sized>(
        &mut self,
        output: &mut [Frame],
        pages: Option<&mut P>,
        observer: Option<&mut crate::NodeSignalObserver<'_>>,
        capture: Option<&mut dyn CaptureSink>,
        sources: Option<crate::native_device_resources::SamplerSourcesRef<'_>>,
        foreign: &mut dyn hex_lattice::ForeignProcessors,
    ) -> Result<(), SessionPlaybackError> {
        self.render_internal_with_live_audio(
            output, pages, observer, capture, sources, foreign, None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn render_internal_with_live_audio<P: MediaPages + ?Sized>(
        &mut self,
        output: &mut [Frame],
        pages: Option<&mut P>,
        observer: Option<&mut crate::NodeSignalObserver<'_>>,
        capture: Option<&mut dyn CaptureSink>,
        sources: Option<crate::native_device_resources::SamplerSourcesRef<'_>>,
        foreign: &mut dyn hex_lattice::ForeignProcessors,
        live_audio: Option<LiveTrackAudio<'_>>,
    ) -> Result<(), SessionPlaybackError> {
        // The runtime keeps this revision throughout rendering. Its cloned
        // handle cannot become the final owner on the callback.
        let revision =
            (!self.revision.foreign_automation.is_empty()).then(|| Arc::clone(&self.revision));
        let mut authored;
        let foreign = if let Some(revision) = revision.as_ref() {
            authored = crate::foreign_automation::WithPrograms {
                programs: &revision.foreign_automation,
                inner: foreign,
            };
            &mut authored as &mut dyn hex_lattice::ForeignProcessors
        } else {
            foreign
        };
        // Lend the per-voice miss-fade table to the page view for this render
        // (`crate::stream_miss_fade`); it is a plain array, so nothing allocates.
        let mut fades = self.stream_miss_fades;
        let result = match pages {
            Some(pages) => {
                let mut faded = crate::stream_miss_fade::MissFadedPages {
                    pages,
                    fades: &mut fades,
                };
                self.render_internal_metered(
                    output,
                    Some(&mut faded),
                    observer,
                    capture,
                    sources,
                    foreign,
                    live_audio,
                )
            }
            None => self.render_internal_metered::<P>(
                output, None, observer, capture, sources, foreign, live_audio,
            ),
        };
        self.stream_miss_fades = fades;
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn render_internal_metered<P: MediaPages + ?Sized>(
        &mut self,
        output: &mut [Frame],
        pages: Option<&mut P>,
        observer: Option<&mut crate::NodeSignalObserver<'_>>,
        capture: Option<&mut dyn CaptureSink>,
        sources: Option<crate::native_device_resources::SamplerSourcesRef<'_>>,
        foreign: &mut dyn hex_lattice::ForeignProcessors,
        live_audio: Option<LiveTrackAudio<'_>>,
    ) -> Result<(), SessionPlaybackError> {
        self.render_work.begin(&self.revision, self.device_metering);
        let result = if self.device_metering {
            // Keep bounded scratch outside the mutable playback borrow; the
            // revision remains owned by playback until scratch is restored.
            let revision = Arc::clone(&self.revision);
            let mut cells = std::mem::take(&mut self.meter_cells);
            let mut meter = super::meter_capture::MeterCapture::new(&revision, &mut cells, capture);
            let result = self.render_on_active_route(
                output,
                pages,
                observer,
                Some(&mut meter),
                sources,
                foreign,
                live_audio,
            );
            if result.is_err() {
                meter.invalidate();
            }
            self.meter_cells = cells;
            result
        } else {
            self.render_on_active_route(
                output, pages, observer, capture, sources, foreign, live_audio,
            )
        };
        self.render_work
            .publish(&self.revision, output.len(), result.is_ok());
        if result.is_err() {
            foreign.reject_callback();
            output.fill([0.0; 2]);
        }
        result
    }

    /// Render only the loaned source and its private downstream history. The
    /// normal meter and capture transactions still belong to the caller.
    pub(super) fn render_resident_source_block(
        &mut self,
        output: &mut [Frame],
        observer: Option<&mut crate::NodeSignalObserver<'_>>,
        capture: Option<&mut dyn CaptureSink>,
        foreign: &mut dyn hex_lattice::ForeignProcessors,
    ) -> Result<(), SessionPlaybackError> {
        self.monitor_source_releases = true;
        let pending_monitor_pcm = self
            .compensation
            .as_ref()
            .is_some_and(|runtime| !runtime.delays.pcm_history_is_silent());
        self.render_internal_borrowed::<dyn MediaPages>(
            output,
            None,
            observer,
            capture,
            None,
            foreign,
            None,
            pending_monitor_pcm,
        )
    }

    pub(super) fn permits_resident_pause_source_monitor(&self) -> bool {
        !self.playing
            && !self.linear_arrangement_export
            && (self.performance_frame != 0 || self.arrangement_frame != 0)
            && self.anchor.is_none()
            && self.transient_loop.is_none()
            && self.current_sampler_keyboards.is_empty()
            && self
                .mutable_instruments
                .iter()
                .flatten()
                .all(MutableInstrumentTrackRuntime::is_source_owner)
            && !self.midi_capture_active()
            && self.tail_groups.iter().all(Option::is_none)
            && self.tail_mutable_instruments.iter().all(Option::is_none)
            && self.releases.iter().all(|release| {
                release.location.tail_group().is_none()
                    && release.location.track_index() < self.routes.len()
            })
            && !self.stopped_edit_tails()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_on_active_route<P: MediaPages + ?Sized>(
        &mut self,
        output: &mut [Frame],
        pages: Option<&mut P>,
        observer: Option<&mut crate::NodeSignalObserver<'_>>,
        capture: Option<&mut dyn CaptureSink>,
        sources: Option<crate::native_device_resources::SamplerSourcesRef<'_>>,
        foreign: &mut dyn hex_lattice::ForeignProcessors,
        live_audio: Option<LiveTrackAudio<'_>>,
    ) -> Result<(), SessionPlaybackError> {
        if self.defer_paused_source_monitor
            && !self.playing
            && (self.performance_frame != 0 || self.arrangement_frame != 0)
            && live_audio.is_none()
        {
            // Includes the suffix of a callback that just completed a Pause
            // fade. Never spend original graph history while the host waits
            // for private effects or after their activation is refused.
            output.fill([0.0; 2]);
            if let Some(capture) = capture {
                capture.commit(output.len());
            }
            return Ok(());
        }
        if !output.is_empty()
            && let Some(mut monitor) = self.paused_source_monitor.take()
        {
            if live_audio.is_none() && self.permits_resident_pause_source_monitor() {
                let result = monitor.render_resident(self, output, observer, capture, foreign);
                self.paused_source_monitor = Some(monitor);
                return result;
            }
            monitor.end_resident_interval();
            self.paused_source_monitor = Some(monitor);
        }
        self.render_internal_borrowed(
            output, pages, observer, capture, sources, foreign, live_audio, false,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn render_internal_borrowed<P: MediaPages + ?Sized>(
        &mut self,
        output: &mut [Frame],
        pages: Option<&mut P>,
        observer: Option<&mut crate::NodeSignalObserver<'_>>,
        mut capture: Option<&mut dyn CaptureSink>,
        sources: Option<crate::native_device_resources::SamplerSourcesRef<'_>>,
        foreign: &mut dyn hex_lattice::ForeignProcessors,
        live_audio: Option<LiveTrackAudio<'_>>,
        pending_monitor_pcm: bool,
    ) -> Result<(), SessionPlaybackError> {
        if let Some(state) = self.native_parameter_state.as_deref_mut() {
            state.application = None;
            state.processing = state.input.filter(|_| self.playing).map(|input| {
                crate::native_parameter_input::Processing::new(
                    input,
                    self.performance_frame,
                    self.revision.arrangement().main_latency_frames(),
                )
            });
        }
        self.begin_midi_capture_render(output.len());
        if self.revision.resource_scope.is_some()
            && self.revision.resource_scope != sources.map(|s| s.id())
        {
            self.fail_midi_capture_render();
            return Err(SessionPlaybackError::MutableInstrumentDiagnostic(
                "sampler requires its exact source scope",
            ));
        }
        output.fill([0.0; 2]);
        let stopped_edit_tails = self.stopped_edit_tails();
        let pending_monitor_pcm = pending_monitor_pcm
            || (live_audio.is_none()
                && self.stopped_input_has_history()
                && self
                    .compensation
                    .as_ref()
                    .is_some_and(|runtime| !runtime.delays.pcm_history_is_silent()));
        if output.is_empty()
            || (live_audio.is_none() && self.renders_no_device_chains())
            || (!self.playing
                && live_audio.is_none()
                && !pending_monitor_pcm
                && !self.foreign_arrangement.has_started()
                && !self.foreign_arrangement.has_keyboard()
                && self.computer_keyboard.is_none()
                && self.current_sampler_keyboards.is_empty()
                && !self
                    .return_effect_tails
                    .slots()
                    .iter()
                    .flatten()
                    .any(|tail| tail.output_active)
                && self
                    .mutable_instruments
                    .iter()
                    .flatten()
                    .all(|runtime| runtime.committed_is_quiescent())
                && !self.has_analytic_release_output()
                && !self.source_effects.has_monitor_tail()
                && self
                    .device_chains
                    .iter()
                    .all(|chain| chain.committed_range_is_quiescent(0..chain.len()))
                && self
                    .return_chains
                    .iter()
                    .all(|chain| chain.committed_range_is_quiescent(0..chain.len()))
                && self
                    .main_chain
                    .committed_range_is_quiescent(0..self.main_chain.len())
                && !self.tail_output_active.iter().any(|active| *active)
                && self
                    .tail_mutable_instruments
                    .iter()
                    .flatten()
                    .all(|tail| tail.runtime.committed_is_quiescent()))
        {
            if let Some(capture) = capture.as_mut() {
                capture.commit(output.len());
            }
            return Ok(());
        }
        // A handoff can occupy a sparse set of slots. Rendering may drain
        // them, but only a handoff can install a new detached owner.
        self.detached_tail_group_limit = self
            .tail_groups
            .iter()
            .zip(&self.tail_mutable_instruments)
            .zip(&self.tail_output_active)
            .rposition(|((chain, instrument), active)| {
                chain.is_some() || instrument.is_some() || *active
            })
            .map_or(0, |index| index + 1);
        let selected = self
            .live_expression
            .as_ref()
            .map(|input| (input.track, input.instrument, input.snapshot()));
        let source_selected = self.live_expression.as_ref().and_then(|input| {
            input
                .published_snapshot()
                .map(|snapshot| (input.track, input.instrument, snapshot))
        });
        for (route, runtime) in self.routes.iter().zip(self.mutable_instruments.iter_mut()) {
            if let Some(runtime) = runtime {
                runtime.expression = match if runtime.is_source_owner() {
                    source_selected
                } else {
                    selected
                } {
                    Some((track, instrument, snapshot))
                        if (track == route.id
                            || self.revision.source_routing().receiver(track)
                                == Some(route.id))
                            && runtime.expression_identity == Some(instrument) =>
                    {
                        Some(snapshot)
                    }
                    _ => runtime.expression,
                };
            }
        }
        for tail in self.tail_mutable_instruments.iter_mut().flatten() {
            tail.runtime.expression = match if tail.runtime.is_source_owner() {
                source_selected
            } else {
                selected
            } {
                Some((track, instrument, snapshot))
                    if track == tail.track_id
                        && tail.runtime.expression_identity == Some(instrument) =>
                {
                    Some(snapshot)
                }
                _ => tail.runtime.expression,
            };
        }
        if let Some(keyboard) = self.computer_keyboard.as_mut() {
            keyboard.set_expression(selected);
        }
        let mut observer = observer;
        if let Some(compensation) = self.compensation.as_mut()
            && compensation.delays.begin_transaction().is_err()
        {
            self.fail_midi_capture_render();
            return Err(SessionPlaybackError::DeviceRuntime);
        }
        if let Some(keyboard) = &mut self.computer_keyboard {
            keyboard.begin_block();
        }
        for keyboard in &mut self.current_sampler_keyboards {
            keyboard.begin_block();
        }
        self.foreign_arrangement.begin();
        let main_tails_only = !self.playing
            && live_audio.is_none()
            && !self.foreign_arrangement.has_started()
            && !self.foreign_arrangement.has_keyboard()
            && stopped_edit_tails
            && !self.monitor_source_releases
            && !self.stopped_input_has_history()
            && self.computer_keyboard.is_none()
            && self.current_sampler_keyboards.is_empty()
            && !self
                .return_effect_tails
                .slots()
                .iter()
                .flatten()
                .any(|tail| tail.output_active && tail.return_id().is_some());
        let ordinary_monitor = !self.playing && !main_tails_only;
        let audition_before = self.browser_audition.voices;
        let track_count = self.routes.len();
        self.track_fader_scratch[..track_count].copy_from_slice(&self.track_faders[..track_count]);
        self.live_gain_scratch = self.live_gain;
        let result = if self.playing {
            self.render_playing(
                output,
                pages,
                &mut observer,
                &mut capture,
                sources,
                foreign,
                live_audio,
            )
        } else if main_tails_only {
            // No admitted source monitoring can still feed the current chain.
            // Its flag survives input removal until an explicit discontinuity.
            // Detached Main output has no input for the new current chain.
            // Drain only its original owners; do not advance a newly restored
            // plugin's DSP clock by feeding it invented monitoring callbacks.
            self.render_stopped_main_tails(output, foreign)
        } else {
            self.render_keyboard_monitor(
                output,
                &mut observer,
                &mut capture,
                sources,
                foreign,
                live_audio,
            )
        };
        if result.is_err() {
            self.browser_audition.voices = audition_before;
            self.fail_midi_capture_render();
            for chain in self
                .device_chains
                .iter_mut()
                .chain(self.source_effects.chains_mut())
                .chain(self.return_chains.iter_mut())
                .chain(std::iter::once(&mut self.main_chain))
            {
                chain.rollback_block();
            }
            for chain in self.tail_groups.iter_mut().flatten() {
                chain.rollback_block();
            }
            if let Some(keyboard) = &mut self.computer_keyboard {
                keyboard.rollback();
            }
            for keyboard in &mut self.current_sampler_keyboards {
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
            if let Some(compensation) = self.compensation.as_mut()
                && compensation.delays.commit_transaction().is_err()
            {
                self.browser_audition.voices = audition_before;
                self.fail_midi_capture_render();
                if let Some(keyboard) = &mut self.computer_keyboard {
                    keyboard.rollback();
                }
                for keyboard in &mut self.current_sampler_keyboards {
                    keyboard.rollback();
                }
                output.fill([0.0; 2]);
                return Err(SessionPlaybackError::DeviceRuntime);
            }
            self.foreign_arrangement.commit();
            self.track_faders[..track_count]
                .copy_from_slice(&self.track_fader_scratch[..track_count]);
            self.live_gain = self.live_gain_scratch;
            if ordinary_monitor {
                // Main-only draining never staged these source/tail buffers.
                std::mem::swap(&mut self.releases, &mut self.release_scratch);
                self.tail_output_active
                    .copy_from_slice(&self.tail_output_scratch);
            }
            if let Some(keyboard) = &mut self.computer_keyboard {
                keyboard.commit();
            }
            for keyboard in &mut self.current_sampler_keyboards {
                keyboard.commit();
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
        if result.is_ok() {
            if let Some(state) = self.native_parameter_state.as_deref_mut() {
                state.application = state.processing.as_ref().and_then(|input| input.applied);
            }
            self.seal_midi_capture_render();
        }
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn render_playing<P: MediaPages + ?Sized>(
        &mut self,
        output: &mut [Frame],
        pages: Option<&mut P>,
        observer: &mut Option<&mut crate::NodeSignalObserver<'_>>,
        capture: &mut Option<&mut dyn CaptureSink>,
        sources: Option<crate::native_device_resources::SamplerSourcesRef<'_>>,
        foreign: &mut dyn hex_lattice::ForeignProcessors,
        live_audio: Option<LiveTrackAudio<'_>>,
    ) -> Result<(), SessionPlaybackError> {
        // Move only the existing scratch Box, then restore it on every result.
        // No callback allocation, new owner, or final source/graph drop occurs.
        let mut scalar = self
            .native_parameter_state
            .as_deref_mut()
            .and_then(|state| state.scalar.take());
        let result = self.render_playing_with_scalar(
            output,
            pages,
            observer,
            capture,
            sources,
            foreign,
            scalar.as_deref_mut(),
            live_audio,
        );
        if let Some(state) = self.native_parameter_state.as_deref_mut() {
            state.scalar = scalar;
        }
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn render_playing_with_scalar<P: MediaPages + ?Sized>(
        &mut self,
        output: &mut [Frame],
        mut pages: Option<&mut P>,
        observer: &mut Option<&mut crate::NodeSignalObserver<'_>>,
        capture: &mut Option<&mut dyn CaptureSink>,
        sources: Option<crate::native_device_resources::SamplerSourcesRef<'_>>,
        foreign: &mut dyn hex_lattice::ForeignProcessors,
        mut scalar: Option<&mut crate::scalar_automation::StateSet>,
        live_audio: Option<LiveTrackAudio<'_>>,
    ) -> Result<(), SessionPlaybackError> {
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
        let mut resident_analytic_eligible = [false; MAX_TRACKS];
        let mut aligned = [false; MAX_TRACKS];
        let mut passthrough = [compensated::ResidentRoute::Scalar; MAX_TRACKS];
        let mut stamps = [None; 256];
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
                let result = self.apply_due_routes(performance_frame, arrangement_frame)?;
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
                self.prepare_resident_sources(
                    performance_frame,
                    arrangement_frame,
                    count,
                    &mut resident_source_eligible,
                    &mut resident_analytic_eligible,
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
                    use_resident_sources.then_some(compensated::BatchedSources {
                        index: resident_source_index,
                        audio: &resident_source_eligible,
                        analytic: &resident_analytic_eligible,
                        stamps: projected_history.map(|_| &stamps[..resident_source_end]),
                        aligned: &aligned,
                        passthrough: &passthrough,
                        batched_sends,
                    }),
                    live_audio,
                )?;
                if use_resident_sources {
                    resident_source_index += 1;
                }
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
                    sources,
                    foreign,
                    external,
                    live_audio,
                )?;
            }
            if current_first_frame {
                for route in &mut self.route_scratch {
                    route.current_first_frame = false;
                }
            }
            if self.compensation.is_some() && !upstream_frame_routing {
                // Only Main needs delayed content clocks. Keep upstream work
                // batched, then advance the existing stamp/Main/tail owners in
                // exactly the original sample order, inside this transaction.
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
                        self.compensation.as_ref(),
                        &clock,
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
                    self.compensation.as_ref(),
                    &tail_clock,
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
                self.owner_for_target(pending.target, target_beat, performance_frame)?
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
        capture: &mut Option<&mut dyn CaptureSink>,
        sources: Option<crate::native_device_resources::SamplerSourcesRef<'_>>,
        foreign: &mut dyn hex_lattice::ForeignProcessors,
        external: Option<&dyn hex_lattice::NativeExternalAutomationSet>,
        live_audio: Option<LiveTrackAudio<'_>>,
    ) -> Result<(), SessionPlaybackError> {
        let region = self.active_loop_region();
        let loop_fade = region.and_then(|region| {
            crate::loop_seam::LoopEndFade::new(region, self.revision.arrangement().sample_rate())
        });
        let revision = Arc::clone(&self.revision);
        let tracks = revision.arrangement().tracks();
        for scratch in &mut self.return_scratch {
            scratch[..output.len()].fill([0.0; 2]);
        }
        let timing = timing_view_for(self.revision.session(), self.periodic_tempo(), self.anchor);
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
            let work_start = self.render_work.track(index);
            let track = tracks
                .get(index)
                .ok_or(SessionPlaybackError::UnknownTrack)?;
            let capture_id = OutputCaptureId::Track(self.route_scratch[index].id);
            let recording_track =
                self.revision.gain_recording_track() == Some(self.route_scratch[index].id);
            let recording_override = self.gain_recording_override.filter(|_| recording_track);
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
            let audition = self
                .browser_audition
                .voices
                .for_track(self.route_scratch[index].id);
            let live_target = self.computer_keyboard.as_ref().is_some_and(|keyboard| {
                !keyboard.is_source_input() && keyboard.track() == self.route_scratch[index].id
            }) || self.source_input_receiver() == Some(index)
                || !audition.is_empty()
                || live_audio.is_some_and(|input| input.track == self.route_scratch[index].id);
            let live_target = live_target
                || self
                    .current_sampler_keyboards
                    .iter()
                    .any(|k| k.track() == self.route_scratch[index].id);
            let shared_source = revision.source_routing().is_shared(index);
            let smoothing = self.track_fader_scratch[index].active()
                && !track.has_gain_automation()
                && recording_override.is_none();
            let has_source_effects = self.source_effects.has_chain(index);
            let buffered = has_source_effects
                || shared_source
                || live_target
                || stateful_effects
                || mutable_active
                || !track.sends().is_empty()
                || track.has_gain_automation()
                || smoothing
                || self.device_metering;
            let ordered = has_source_effects
                || shared_source
                || live_target
                || smoothing
                || track.requires_ordered_processing();
            let already_mixed = self.device_metering
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
            let has_current_route = (shared_source
                && shared_source_has_route(&self.revision, &self.route_scratch, index)?)
                || mutable_active
                || live_target
                || !matches!(self.route_scratch[index].owner, RouteOwner::Silence)
                || self.route_scratch[index].audio_release.is_some()
                || !matches!(midi_route.owner, RouteOwner::Silence);
            let has_current_effect_tail = !has_current_route
                && !has_current_release
                && (self.source_effects.has_tail(index)
                    || self
                        .device_chains
                        .get(index)
                        .is_some_and(|chain| !chain.committed_range_is_quiescent(0..chain.len())));
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
                            midi: crate::session_inventory::EffectiveSessionMidi::new(
                                self.revision.session(),
                                self.midi_admission.as_ref(),
                            ),
                            route: self.route_scratch[index],
                            historical_floor: self.historical_floor_for(index),
                            suppressions: &self.suppression_scratch,
                            continuations: &self.analytic_continuations,
                            index,
                            raw: has_source_effects || live_target || shared_source || smoothing,
                            loop_fade,
                            timing: &timing,
                            table,
                        },
                        &mut self.spans,
                        performance_frame,
                        arrangement_frame,
                        render_output,
                        pages.as_deref_mut(),
                        SourceStage::audio(audition),
                    )?;
                    if let Some(input) =
                        live_audio.filter(|input| input.track == self.route_scratch[index].id)
                    {
                        for (frame, source) in render_output
                            .iter_mut()
                            .zip(&input.frames[frame_offset..frame_offset + output.len()])
                        {
                            frame[0] += source[0];
                            frame[1] += source[1];
                        }
                    }
                    if stateful_effects {
                        crate::native_parameter_input::process_range_with_external(
                            self.native_parameter_state
                                .as_deref_mut()
                                .and_then(|state| state.processing.as_mut()),
                            external,
                            &mut self.device_chains[index],
                            0..track.instrument_position(),
                            render_output,
                            frame_offset,
                            Some(&effect_clock),
                            Some(&effect_clock),
                            0,
                            0,
                            &mut hex_lattice::RefuseForeign,
                            observer,
                        )
                        .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
                    } else {
                        track.apply_ordered_prefix(render_output);
                    }
                    track.apply_instrument_input_gain(render_output);
                    if self.device_metering {
                        track.instrument_meter.observe_input(render_output);
                    }
                    // For a prepared source unit, isolate the complete generator
                    // sum from the receiver's own clips before its effects run.
                    let mut generator_buffer;
                    let mut isolated = if has_source_effects {
                        generator_buffer = [[0.0; 2]; 256];
                        Some(&mut generator_buffer[..output.len()])
                    } else {
                        None
                    };
                    let generator = isolated.as_deref_mut().unwrap_or(&mut *render_output);
                    for release in &self.release_scratch[first_release..release_cursor] {
                        if release.location.tail_group().is_none() {
                            render_release_raw(
                                release,
                                table,
                                self.revision.arrangement().shared_contexts(),
                                &self.retained_contexts,
                                &self.analytic_continuations,
                                performance_frame,
                                self.playing,
                                generator,
                            )?;
                        }
                    }
                    render_source_instrument_chunk(
                        RouteSourceContext {
                            revision: &self.revision,
                            midi: crate::session_inventory::EffectiveSessionMidi::new(
                                self.revision.session(),
                                self.midi_admission.as_ref(),
                            ),
                            route: midi_route,
                            historical_floor: self.historical_floor_for(index),
                            suppressions: &self.suppression_scratch,
                            continuations: &self.analytic_continuations,
                            index,
                            raw: has_source_effects || live_target || shared_source || smoothing,
                            loop_fade,
                            timing: &timing,
                            table,
                        },
                        &self.route_scratch,
                        self.historical_floor,
                        &self.route_historical_floors,
                        &mut self.spans,
                        performance_frame,
                        arrangement_frame,
                        generator,
                    )?;
                    if self.source_input_receiver() == Some(index) {
                        let authored = self
                            .revision
                            .source_routing()
                            .contributors(index)
                            .iter()
                            .try_fold(0usize, |sum, &contributor| {
                                sum.checked_add(
                                    self.source_voice_envelope(usize::from(contributor)),
                                )
                            })
                            .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
                        source_live::render_source_live_input(
                            &self.revision,
                            self.computer_keyboard.as_mut().map(Box::as_mut),
                            self.mutable_instruments[index].as_mut(),
                            &mut self.mutable_onsets,
                            &mut self.mutable_events,
                            authored,
                            performance_frame,
                            generator,
                            &timing,
                            true,
                            None,
                            self.pending_shared_live,
                        )?;
                    }
                    render_mutable_instrument_chunk(
                        MutableInstrumentChunkContext {
                            revision: &self.revision,
                            midi: crate::session_inventory::EffectiveSessionMidi::new(
                                self.revision.session(),
                                self.midi_admission.as_ref(),
                            ),
                            sources,
                            routes: &self.route_scratch,
                            runtimes: &mut self.mutable_instruments,
                            onsets: &mut self.mutable_onsets,
                            suppressions: &self.suppression_scratch,
                            events: &mut self.mutable_events,
                            spans: &mut self.spans,
                            performance_frame,
                            arrangement_frame,
                            arrangement_loop_pass,
                            transient: None,
                            chase_at_start: self.interactive_start_frame == Some(performance_frame),
                            pending_shared_chase: self.pending_shared_chase,
                            loop_region: if self.linear_arrangement_export {
                                None
                            } else {
                                region.filter(|region| region.enabled())
                            },
                            timing: &timing,
                        },
                        index,
                        generator,
                        self.native_parameter_state
                            .as_deref_mut()
                            .and_then(|state| state.processing.as_mut()),
                    )?;
                    if let Some(keyboard) = self.computer_keyboard.as_mut().filter(|keyboard| {
                        !keyboard.is_source_input()
                            && keyboard.track() == self.route_scratch[index].id
                    }) {
                        let clock = keyboard
                            .musical_clock(&timing, performance_frame, true)
                            .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                        keyboard
                            .render(generator, Some(&clock))
                            .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                    }
                    for keyboard in self
                        .current_sampler_keyboards
                        .iter_mut()
                        .filter(|k| k.track() == self.route_scratch[index].id)
                    {
                        let source = sources.ok_or(SessionPlaybackError::DeviceRuntime)?;
                        let resources = source
                            .resources(keyboard.track())
                            .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
                        keyboard
                            .render_with_resources(generator, None, &resources)
                            .map_err(|e| SessionPlaybackError::Arrangement(e.0))?;
                    }
                    if let Some(generated) = isolated {
                        self.source_effects
                            .process_with_live_input(
                                index,
                                generated,
                                &effect_clock,
                                frame_offset,
                                self.native_parameter_state
                                    .as_deref_mut()
                                    .and_then(|state| state.processing.as_mut()),
                            )
                            .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                        for (target, source) in render_output.iter_mut().zip(generated) {
                            target[0] += source[0];
                            target[1] += source[1];
                        }
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
                        crate::native_parameter_input::process_range_with_external(
                            self.native_parameter_state
                                .as_deref_mut()
                                .and_then(|state| state.processing.as_mut()),
                            external,
                            chain,
                            track.instrument_position()..effect_count,
                            render_output,
                            frame_offset,
                            Some(&effect_clock),
                            Some(&effect_clock),
                            0,
                            0,
                            &mut hex_lattice::RefuseForeign,
                            observer,
                        )
                        .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
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
                                    self.playing,
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
                                self.playing,
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
                                midi: crate::session_inventory::EffectiveSessionMidi::new(
                                    self.revision.session(),
                                    self.midi_admission.as_ref(),
                                ),
                                route: self.route_scratch[index],
                                historical_floor: self.historical_floor_for(index),
                                suppressions: &self.suppression_scratch,
                                continuations: &self.analytic_continuations,
                                index,
                                raw: false,
                                loop_fade,
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
                            midi: crate::session_inventory::EffectiveSessionMidi::new(
                                self.revision.session(),
                                self.midi_admission.as_ref(),
                            ),
                            route: self.route_scratch[index],
                            historical_floor: self.historical_floor_for(index),
                            suppressions: &self.suppression_scratch,
                            continuations: &self.analytic_continuations,
                            index,
                            raw: false,
                            loop_fade,
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
                    let range = 0..chain.len();
                    crate::native_parameter_input::process_range_with_external(
                        self.native_parameter_state
                            .as_deref_mut()
                            .and_then(|state| state.processing.as_mut()),
                        external,
                        chain,
                        range,
                        render_output,
                        frame_offset,
                        Some(&effect_clock),
                        Some(&effect_clock),
                        0,
                        0,
                        &mut hex_lattice::RefuseForeign,
                        observer,
                    )
                    .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
                }
                for (frame_index, source) in render_output.iter_mut().enumerate() {
                    let content_frame = arrangement_frame
                        .checked_add(frame_index as u64)
                        .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
                    let live_gain = if recording_track {
                        self.live_gain_scratch.next(
                            track,
                            content_frame,
                            recording_override,
                            self.revision.arrangement().sample_rate(),
                        )
                    } else {
                        None
                    };
                    let mix = if let Some(gain) = live_gain {
                        track.recording_mix(gain)
                    } else if track.has_gain_automation() {
                        track.mix_at(content_frame)
                    } else {
                        track.mix()
                    };
                    let mix = if track.has_gain_automation() || recording_override.is_some() {
                        self.track_fader_scratch[index].follow(mix.channel_gain());
                        mix
                    } else if smoothing {
                        mix.with_channel_gain(self.track_fader_scratch[index].next())
                    } else {
                        mix
                    };
                    let mix = if live_target {
                        mix.without_effects()
                    } else {
                        mix
                    };
                    let contribution = if already_mixed {
                        // Preserve the analytic per-voice fader summation order.
                        output[frame_index][0] += source[0];
                        output[frame_index][1] += source[1];
                        *source
                    } else if track.sends().is_empty() {
                        mix.add_frame(&mut output[frame_index], *source);
                        mix.apply_frame(*source)
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
                        contribution
                    };
                    if capture.is_some() || self.device_metering {
                        *source = contribution;
                    }
                }
                if let Some(capture) = capture.as_mut() {
                    capture.add(capture_id, frame_offset, render_output)?;
                }
            } else {
                let selected = capture
                    .as_ref()
                    .is_some_and(|capture| capture.selects(capture_id))
                    && matches!(self.route_scratch[index].owner, RouteOwner::Arrangement);
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
                                self.playing,
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
                            Some(self.revision.session().timing()),
                            loop_fade,
                        )
                        .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                    if let Some(capture) = capture.as_mut() {
                        capture.add(capture_id, frame_offset, &captured[..output.len()])?;
                    }
                    self.render_work.finish(work_start);
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
                            self.playing,
                            output,
                            None,
                        )?;
                    }
                }
                render_route_source_chunk(
                    RouteSourceContext {
                        revision: &self.revision,
                        midi: crate::session_inventory::EffectiveSessionMidi::new(
                            self.revision.session(),
                            self.midi_admission.as_ref(),
                        ),
                        route: self.route_scratch[index],
                        historical_floor: self.historical_floor_for(index),
                        suppressions: &self.suppression_scratch,
                        continuations: &self.analytic_continuations,
                        index,
                        raw: false,
                        loop_fade,
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
            self.render_work.finish(work_start);
        }
        self.render_detached_tails(
            performance_frame,
            output,
            frame_offset,
            capture,
            None,
            None,
            foreign,
        )?;
        self.render_return_buses(
            output,
            frame_offset,
            Some(&effect_clock),
            observer,
            capture,
            foreign,
            external,
        )
    }

    #[allow(clippy::too_many_arguments)] // Existing return-stage borrows plus recorded values.
    fn render_return_buses(
        &mut self,
        output: &mut [Frame],
        frame_offset: usize,
        effect_clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
        observer: &mut Option<&mut crate::NodeSignalObserver<'_>>,
        capture: &mut Option<&mut dyn CaptureSink>,
        foreign: &mut dyn hex_lattice::ForeignProcessors,
        external: Option<&dyn hex_lattice::NativeExternalAutomationSet>,
    ) -> Result<(), SessionPlaybackError> {
        let arrangement = self.revision.arrangement();
        if !arrangement.return_feed_plan.is_empty() {
            for &index in arrangement.return_feed_plan.order() {
                let work_start = self.render_work.return_bus(index);
                let bus = &arrangement.return_buses()[index];
                let frames = &mut self.return_scratch[index][..output.len()];
                let chain = self
                    .return_chains
                    .get_mut(index)
                    .ok_or(SessionPlaybackError::DeviceRuntime)?;
                let range = 0..chain.len();
                crate::native_parameter_input::process_range_with_external(
                    self.native_parameter_state
                        .as_deref_mut()
                        .and_then(|state| state.processing.as_mut()),
                    external,
                    chain,
                    range,
                    frames,
                    frame_offset,
                    effect_clock,
                    effect_clock,
                    0,
                    0,
                    foreign,
                    observer,
                )
                .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
                if let Some(matrix) = bus.matrix {
                    matrix.apply_block(frames);
                }
                if bus.muted {
                    frames.fill([0.0; 2]);
                } else {
                    for frame in frames.iter_mut() {
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
                self.render_work.finish(work_start);
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
                    .render_chunk(output, frame_offset, capture, foreign)?;
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
            let work_start = self.render_work.return_bus(index);
            let frames = self
                .return_scratch
                .get_mut(index)
                .ok_or(SessionPlaybackError::DeviceRuntime)?;
            let frames = &mut frames[..output.len()];
            let chain = self
                .return_chains
                .get_mut(index)
                .ok_or(SessionPlaybackError::DeviceRuntime)?;
            let range = 0..chain.len();
            crate::native_parameter_input::process_range_with_external(
                self.native_parameter_state
                    .as_deref_mut()
                    .and_then(|state| state.processing.as_mut()),
                external,
                chain,
                range,
                frames,
                frame_offset,
                effect_clock,
                effect_clock,
                0,
                0,
                foreign,
                observer,
            )
            .map_err(|_| SessionPlaybackError::DeviceRuntime)?;
            if let Some(matrix) = bus.matrix {
                matrix.apply_block(frames);
            }
            if !bus.muted {
                for (destination, source) in output.iter_mut().zip(frames.iter()) {
                    destination[0] += source[0] * bus.gain;
                    destination[1] += source[1] * bus.gain;
                }
            }
            if capture.is_some() || self.device_metering {
                for frame in frames.iter_mut() {
                    *frame = if bus.muted {
                        [0.0; 2]
                    } else {
                        [frame[0] * bus.gain, frame[1] * bus.gain]
                    };
                }
            }
            if let Some(capture) = capture.as_mut() {
                capture.add(OutputCaptureId::Return(bus.id), frame_offset, frames)?;
            }
            self.render_work.finish(work_start);
        }
        if self.playing {
            self.return_effect_tails
                .render_chunk(output, frame_offset, capture, foreign)?;
        }
        Ok(())
    }
    #[allow(clippy::too_many_arguments)] // Both bounded routing buffers share the render context.
    fn render_detached_tails(
        &mut self,
        performance_frame: u64,
        output: &mut [Frame],
        frame_offset: usize,
        capture: &mut Option<&mut dyn CaptureSink>,
        mut routed: Option<&mut [Frame]>,
        mut routed_sends: Option<&mut [Frame]>,
        foreign: &mut dyn hex_lattice::ForeignProcessors,
    ) -> Result<(), SessionPlaybackError> {
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
                    if output.len() != 1 {
                        return Err(SessionPlaybackError::DeviceRuntime);
                    }
                    render_release_captured(
                        release,
                        table,
                        self.revision.arrangement().shared_contexts(),
                        &self.retained_contexts,
                        &self.analytic_continuations,
                        performance_frame,
                        self.playing,
                        &mut captured[..1],
                        None,
                    )?;
                    let index = graph
                        .and_then(|graph| graph.track_index(release.track_id))
                        .ok_or(SessionPlaybackError::UnknownTrack)?;
                    let destination = destinations
                        .get_mut(index)
                        .ok_or(SessionPlaybackError::UnknownTrack)?;
                    destination[0] += captured[0][0];
                    destination[1] += captured[0][1];
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
                    table,
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
                if !tail.runtime.speculative_is_quiescent() {
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

/// Contribute a detached source to each retained post-fader send before
/// the ordinary Return owner processes its complete input once.
fn add_detached_source_sends(
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
fn emit_detached_tail(
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
        if tail.len() != 1 {
            return Err(SessionPlaybackError::DeviceRuntime);
        }
        let owner = owner.ok_or(SessionPlaybackError::DeviceRuntime)?;
        let index = graph
            .and_then(|graph| graph.track_index(owner))
            .ok_or(SessionPlaybackError::UnknownTrack)?;
        let destination = destinations
            .get_mut(index)
            .ok_or(SessionPlaybackError::UnknownTrack)?;
        mix.add_frame(destination, tail[0]);
        if allow_sends {
            let send_destination = routed_sends
                .as_deref_mut()
                .and_then(|destinations| destinations.get_mut(index))
                .ok_or(SessionPlaybackError::UnknownTrack)?;
            mix.add_frame(send_destination, tail[0]);
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

fn process_mutable_tail<T: MidiTiming + ?Sized>(
    tail: &mut super::MutableInstrumentTail,
    onsets: &mut MutableOnsetBuffer,
    events: &mut Vec<hex_lattice::NativeInstrumentEvent>,
    performance_frame: u64,
    output: &mut [Frame],
    timing: &T,
    playing: bool,
) -> Result<(), SessionPlaybackError> {
    onsets.clear();
    tail.runtime
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
                        Some(tail.track_id),
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
}

struct MutableInstrumentChunkContext<'a, T: ?Sized> {
    sources: Option<crate::native_device_resources::SamplerSourcesRef<'a>>,
    revision: &'a PreparedRevision,
    midi: crate::session_inventory::EffectiveSessionMidi<'a>,
    routes: &'a [super::TrackRoute],
    runtimes: &'a mut [Option<MutableInstrumentTrackRuntime>],
    onsets: &'a mut MutableOnsetBuffer,
    suppressions: &'a [MidiOccurrenceSuppression],
    events: &'a mut Vec<hex_lattice::NativeInstrumentEvent>,
    spans: &'a mut MidiSpanBuffer,
    performance_frame: u64,
    arrangement_frame: u64,
    arrangement_loop_pass: u64,
    transient: Option<hex_lattice::NativeTransientInput>,
    chase_at_start: bool,
    pending_shared_chase: Option<super::PendingSharedChase>,
    loop_region: Option<hex_project::LoopRegion>,
    timing: &'a T,
}

/// Gather canonical Console note onsets for an already selected MIDI route.
/// Both native and foreign generators consume the same admitted clip template.
pub(super) fn collect_console_midi_onsets<T: MidiTiming + ?Sized>(
    midi: &crate::session_inventory::EffectiveSessionMidi<'_>,
    route: TrackRoute,
    timing: &T,
    start: u64,
    end: u64,
    spans: &mut MidiSpanBuffer,
    onsets: &mut MutableOnsetBuffer,
) -> Result<(), SessionPlaybackError> {
    let RouteOwner::Session { clip, origin } = route.owner else {
        return Err(SessionPlaybackError::InvalidCommand);
    };
    let prepared = midi
        .midi_clip_for_track(clip, route.id)
        .ok_or(SessionPlaybackError::UnknownClip)?;
    if prepared.track != route.id {
        return Err(SessionPlaybackError::ClipTrackMismatch);
    }
    if !prepared.instrument_enabled() {
        return Ok(());
    }
    prepared
        .instrument_template()
        .query_with_origin(origin, timing, start, end, spans)
        .map_err(SessionPlaybackError::Midi)?;
    for span in spans.spans() {
        onsets
            .push(crate::mutable_instrument::MutableNoteOnset {
                key: crate::mutable_instrument::MutableOccurrenceKey::new(
                    prepared.track,
                    span.clip,
                    span.note,
                    span.iteration,
                    route.current_incarnation,
                ),
                onset_beat: span.onset_beat,
                gate_end_beat: span.gate_end_beat,
                source_start: span.start_frame,
                source_gate_end: span.gate_end_frame,
                pitch: span.pitch,
                channel: span.channel,
                velocity: span.velocity,
                contributor_fade_in: false,
            })
            .map_err(|_| SessionPlaybackError::MutableInstrumentEvents)?;
    }
    Ok(())
}

fn render_mutable_instrument_chunk<T: crate::prepared_tempo::MidiTempoBreaks + ?Sized>(
    mut context: MutableInstrumentChunkContext<'_, T>,
    index: usize,
    output: &mut [Frame],
    processing: Option<&mut crate::native_parameter_input::Processing>,
) -> Result<(), SessionPlaybackError> {
    let origin = (
        context.arrangement_frame,
        context.arrangement_loop_pass,
        context.performance_frame,
    );
    context.transient = processing.as_deref().and_then(|processing| {
        let track = context.revision.arrangement().tracks().get(index)?;
        if output.is_empty()
            || !track.receives_source_audio()
            || context
                .runtimes
                .get(index)
                .and_then(Option::as_ref)
                .is_none()
        {
            return None;
        }
        processing.for_source(track.source_id(), track.instrument_id)
    });
    let applied = context.transient.is_some();
    render_mutable_instrument_inner(context, index, output)?;
    if applied && let Some(processing) = processing {
        processing
            .source_applied(origin.0, origin.1, origin.2)
            .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
    }
    Ok(())
}

fn render_mutable_instrument_inner<T: crate::prepared_tempo::MidiTempoBreaks + ?Sized>(
    context: MutableInstrumentChunkContext<'_, T>,
    index: usize,
    output: &mut [Frame],
) -> Result<(), SessionPlaybackError> {
    if context.revision.source_routing().is_shared(index) {
        return shared_source::render_mutable_source_chunk(context, index, output);
    }
    let MutableInstrumentChunkContext {
        revision,
        midi,
        sources,
        routes,
        runtimes,
        onsets,
        suppressions,
        events,
        spans,
        performance_frame,
        arrangement_frame,
        arrangement_loop_pass,
        transient,
        chase_at_start,
        pending_shared_chase: _,
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
                        let is_start_chase = chase_at_start
                            && span.start < arrangement_frame
                            && span.gate_end > arrangement_frame;
                        let source_start = if is_wrap_chase || is_start_chase {
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
                                contributor_fade_in: false,
                            })
                            .is_err();
                    },
                )
                .map_err(|_| SessionPlaybackError::MutableInstrumentEvents)?;
            if event_error {
                return Err(SessionPlaybackError::MutableInstrumentEvents);
            }
        }
        RouteOwner::Session { .. } => {
            collect_console_midi_onsets(
                &midi,
                midi_route,
                timing,
                performance_frame,
                source_end,
                spans,
                onsets,
            )?;
        }
        RouteOwner::SessionAudio { .. } | RouteOwner::Silence => {}
    }
    let runtime = runtimes
        .get_mut(index)
        .and_then(Option::as_mut)
        .ok_or(SessionPlaybackError::MutableInstrumentRuntime)?;
    let scoped = sources
        .filter(|s| s.has_track(track_id))
        .map(|s| s.resources(track_id))
        .transpose()
        .map_err(|_| {
            SessionPlaybackError::MutableInstrumentDiagnostic(
                "sampler resource scope is unavailable",
            )
        })?;
    let resources: &dyn hex_lattice::DeviceResources = scoped.as_ref().map_or(
        &hex_lattice::EMPTY_DEVICE_RESOURCES as &dyn hex_lattice::DeviceResources,
        |s| s,
    );
    let source_start = match midi_route.owner {
        RouteOwner::Arrangement => arrangement_frame,
        _ => performance_frame,
    };
    let arrangement_volumes = track.instrument.as_ref().and_then(|i| i.volumes.as_ref());
    let console = if let RouteOwner::Session { clip, origin } = midi_route.owner {
        midi.midi_clip_for_track(clip, track_id)
            .map(|prepared| (clip, origin, prepared))
    } else {
        None
    };
    let has_program = match midi_route.owner {
        RouteOwner::Arrangement => arrangement_volumes.is_some(),
        RouteOwner::Session { .. } => console
            .as_ref()
            .is_some_and(|(_, _, prepared)| prepared.volume_program().is_some()),
        _ => false,
    };
    let parameter_context = instrument_automation::SourceAutomation::new(
        revision,
        index,
        instrument_automation::Origin {
            arrangement: Some(arrangement_frame),
            performance: performance_frame,
            pass: arrangement_loop_pass,
            running: true,
        },
        output.len(),
    )?;
    let parameter_block = parameter_context.block_with_input(transient)?;
    let parameter_values = parameter_block
        .as_ref()
        .map(|values| values as &dyn hex_lattice::NativeParameterAutomation);
    let clock = InstrumentClock {
        timing,
        start: performance_frame,
    };
    if !has_program && !runtime.has_volume_gain() {
        return runtime
            .process_segment_with_resources(
                track_id,
                performance_frame,
                source_start,
                output,
                onsets,
                suppressions,
                events,
                Some(&clock),
                resources,
                parameter_values,
            )
            .map_err(crate::mutable_instrument::MutableInstrumentRuntimeError::render_error);
    }
    let console_historical = console
        .as_ref()
        .and_then(|(_, origin, prepared)| prepared.volume_program().map(|p| (*origin, p)))
        .map(|(origin, program)| {
            program.historical_end_gain(
                timing,
                crate::midi_clip_volume::VolumePlacement::Session { origin },
            )
        })
        .transpose()
        .map_err(|_| SessionPlaybackError::MutableInstrumentEvents)?
        .flatten()
        .is_some();
    let console_endpoint = if console_historical {
        None
    } else {
        console
            .as_ref()
            .and_then(|(_, origin, prepared)| prepared.volume_program().map(|p| (*origin, p)))
            .map(|(origin, program)| {
                program.finite_endpoint(
                    timing,
                    crate::midi_clip_volume::VolumePlacement::Session { origin },
                )
            })
            .transpose()
            .map_err(|_| SessionPlaybackError::MutableInstrumentEvents)?
            .flatten()
    };
    let mut lookup = |signature: crate::mutable_instrument::MutableOccurrenceSignature,
                      performance: u64,
                      frames: usize,
                      _retaining_muted: bool| {
        use crate::midi_clip_volume::{VolumePlacement, VolumeRamp};
        use crate::mutable_instrument::{MutableInstrumentRuntimeError as Error, VolumeSlice};
        let key = signature.key();
        let unity = |at| VolumeSlice {
            at,
            terminal: None,
            ramp: VolumeRamp {
                start_frame: 0,
                end_frame: u64::MAX,
                start_gain: 1.0,
                end_gain: 1.0,
                next_boundary: None,
            },
        };
        match midi_route.owner {
            RouteOwner::Arrangement => {
                if key.track != track_id || key.loop_pass != arrangement_loop_pass {
                    return Ok(None);
                }
                // CURRENT deliberately omits ordinary mutable source descriptor
                // owners. Its existing admitted note interval index is the
                // authority: query at this exact authored onset, bounded by the
                // compiled voice-density law, never scan canonical clips/notes.
                // A missing/edited source keeps the outgoing scalar; an exact
                // source with an empty curve explicitly restores unity.
                let (onset, gate, pitch, channel, velocity) = signature.details();
                let first = revision
                    .session()
                    .timing()
                    .beat_to_frame(onset)
                    .map_err(|_| Error::InvalidSourceSpan)?;
                let end = first.checked_add(1).ok_or(Error::Arithmetic)?;
                let mut found = false;
                track
                    .for_each_mutable_midi_span(first, end, |span| {
                        found |= span.clip == key.clip
                            && span.note == key.note
                            && span.iteration == key.iteration
                            && span.onset_beat == onset
                            && span.gate_end_beat == gate
                            && span.pitch == pitch
                            && span.channel == channel
                            && span.velocity == velocity;
                    })
                    .map_err(|_| Error::InvalidSourceSpan)?;
                if !found {
                    return Ok(None);
                }
                let at = performance
                    .checked_sub(performance_frame)
                    .and_then(|offset| arrangement_frame.checked_add(offset))
                    .ok_or(Error::Arithmetic)?;
                arrangement_volumes.and_then(|v| v.clip(key.clip)).map_or(
                    Ok(Some(unity(at))),
                    |row| {
                        let Some(endpoint) = row.endpoint else {
                            return Ok(None);
                        };
                        if at >= endpoint.frame {
                            return Ok(None);
                        }
                        // Arrangement coordinates are content frames. The Console
                        // clock may have an absolute rebase floor after a wrap.
                        row.segment_or_hold(revision.session().timing(), at, frames)
                            .map(|ramp| {
                                Some(VolumeSlice {
                                    at,
                                    ramp,
                                    terminal: Some(endpoint.frame),
                                })
                            })
                            .map_err(|_| Error::InvalidSourceSpan)
                    },
                )
            }
            RouteOwner::Session { clip, origin } => {
                if key.clip != clip || key.loop_pass != routes[index].current_incarnation {
                    return Ok(None);
                }
                let Some(program) = console
                    .as_ref()
                    .and_then(|(_, _, prepared)| prepared.volume_program())
                else {
                    return Ok(Some(unity(performance)));
                };
                if console_historical || console_endpoint.is_some_and(|e| performance >= e.frame) {
                    return Ok(None);
                }
                program
                    .segment_at(
                        timing,
                        VolumePlacement::Session { origin },
                        performance,
                        frames,
                    )
                    .map_err(|_| Error::InvalidSourceSpan)?
                    .map(|ramp| VolumeSlice {
                        at: performance,
                        ramp,
                        terminal: console_endpoint.map(|e| e.frame),
                    })
                    .map(Some)
                    .ok_or(Error::InvalidSourceSpan)
            }
            RouteOwner::SessionAudio { .. } | RouteOwner::Silence => Ok(None),
        }
    };
    runtime
        .process_segment_with_volume(
            track_id,
            performance_frame,
            source_start,
            output,
            onsets,
            suppressions,
            events,
            Some(&clock),
            resources,
            &mut lookup,
            parameter_values,
        )
        .map_err(crate::mutable_instrument::MutableInstrumentRuntimeError::render_error)
}

struct RouteSourceContext<'a, T> {
    revision: &'a PreparedRevision,
    midi: crate::session_inventory::EffectiveSessionMidi<'a>,
    route: super::TrackRoute,
    historical_floor: Option<BeatTime>,
    suppressions: &'a [MidiOccurrenceSuppression],
    continuations: &'a [Option<hex_lattice::AnalyticContinuation>],
    index: usize,
    raw: bool,
    loop_fade: Option<crate::loop_seam::LoopEndFade>,
    timing: &'a T,
    table: &'a hex_devices::SineTable,
}

#[derive(Clone, Copy)]
enum SourceStage {
    Whole,
    Audio,
    Audition(browser_audition::Voices),
    Instrument,
}
impl SourceStage {
    fn audio(audition: browser_audition::Voices) -> Self {
        if audition.is_empty() {
            Self::Audio
        } else {
            Self::Audition(audition)
        }
    }
    fn is_audio(self) -> bool {
        matches!(self, Self::Audio | Self::Audition(_))
    }
}

fn render_resident_session_audio(
    prepared: &crate::session_inventory::PreparedSessionAudioClip,
    audio: &crate::Audio,
    launch_frame: u64,
    performance_frame: u64,
    output: &mut [Frame],
    source_mix: crate::PreparedTrackMix,
) -> Result<(), SessionPlaybackError> {
    let frames = audio.frames();
    for (offset, frame) in output.iter_mut().enumerate() {
        let offset = u64::try_from(offset).map_err(|_| SessionPlaybackError::ArithmeticOverflow)?;
        let absolute = performance_frame
            .checked_add(offset)
            .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
        let Some(elapsed) = absolute.checked_sub(launch_frame) else {
            continue;
        };
        let Some(source_frame) = prepared.source_frame(elapsed) else {
            continue;
        };
        let source_index =
            usize::try_from(source_frame).map_err(|_| SessionPlaybackError::ArithmeticOverflow)?;
        let mut sample = *frames
            .get(source_index)
            .ok_or(SessionPlaybackError::UnknownClip)?;
        if prepared.gain != 1.0 {
            sample[0] *= prepared.gain;
            sample[1] *= prepared.gain;
        }
        source_mix.add_frame(frame, sample);
    }
    Ok(())
}

/// Renders the route's owner, then any outgoing Session audio release it
/// still carries, through the same stage, source mix and page provider.
fn render_route_source_chunk<P: MediaPages + ?Sized, T: crate::prepared_tempo::MidiTempoBreaks>(
    context: RouteSourceContext<'_, T>,
    spans: &mut MidiSpanBuffer,
    performance_frame: u64,
    arrangement_frame: u64,
    output: &mut [Frame],
    mut pages: Option<&mut P>,
    stage: SourceStage,
) -> Result<(), SessionPlaybackError> {
    let (revision, route, index, raw) =
        (context.revision, context.route, context.index, context.raw);
    render_route_owner_chunk(
        context,
        spans,
        performance_frame,
        arrangement_frame,
        output,
        pages.as_deref_mut(),
        stage,
    )?;
    if let SourceStage::Audition(audition) = stage
        && !matches!(route.owner, RouteOwner::Arrangement)
        && let Some(track) = revision.arrangement().tracks().get(index)
        && track.audio_content_enabled()
    {
        audition.add(track, performance_frame, output);
    }
    let Some(release) = route.audio_release else {
        return Ok(());
    };
    if matches!(stage, SourceStage::Instrument) {
        return Ok(());
    }
    let track = revision
        .arrangement()
        .tracks()
        .get(index)
        .ok_or(SessionPlaybackError::UnknownTrack)?;
    if stage.is_audio() && !track.audio_content_enabled() {
        return Ok(());
    }
    let source_mix = if raw {
        crate::PreparedTrackMix::identity()
    } else {
        track.source_mix()
    };
    let mut scratch = [[0.0_f32; 2]; 256];
    let mut offset = 0_usize;
    while offset < output.len() {
        let start = performance_frame
            .checked_add(
                u64::try_from(offset).map_err(|_| SessionPlaybackError::ArithmeticOverflow)?,
            )
            .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
        if !release.active_at(start) {
            break;
        }
        let remaining = usize::try_from(release.end_frame() - start).unwrap_or(usize::MAX);
        let count = (output.len() - offset).min(scratch.len()).min(remaining);
        let frames = &mut scratch[..count];
        frames.fill([0.0; 2]);
        render_session_audio_clip(
            revision,
            route.id,
            release.clip,
            release.launch_frame,
            start,
            frames,
            source_mix,
            pages.as_deref_mut(),
        )?;
        for (step, (frame, sample)) in output[offset..offset + count]
            .iter_mut()
            .zip(frames.iter())
            .enumerate()
        {
            let gain = release.gain_at(start + step as u64);
            frame[0] += sample[0] * gain;
            frame[1] += sample[1] * gain;
        }
        offset += count;
    }
    Ok(())
}

fn render_route_owner_chunk<P: MediaPages + ?Sized, T: crate::prepared_tempo::MidiTempoBreaks>(
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
        midi,
        route,
        historical_floor,
        suppressions,
        continuations,
        index,
        raw,
        loop_fade,
        timing,
        table,
    } = context;
    let track = revision
        .arrangement()
        .tracks()
        .get(index)
        .ok_or(SessionPlaybackError::UnknownTrack)?;
    if stage.is_audio() && !track.audio_content_enabled() {
        return Ok(());
    }
    let source_mix = if raw {
        crate::PreparedTrackMix::identity()
    } else {
        track.source_mix()
    };
    match route.owner {
        RouteOwner::Arrangement => {
            if let SourceStage::Audition(audition) = stage {
                return audition
                    .arrangement(
                        track,
                        arrangement_frame,
                        performance_frame,
                        output,
                        pages,
                        loop_fade,
                    )
                    .map_err(|error| SessionPlaybackError::Arrangement(error.0));
            }
            if let Some(fade) =
                loop_fade.filter(|fade| fade.intersects(arrangement_frame, output.len()))
            {
                if !matches!(stage, SourceStage::Instrument) {
                    if output.len() > 256 {
                        return Err(SessionPlaybackError::DeviceRuntime);
                    }
                    let mut scratch = [[0.0; 2]; 256];
                    let audio = &mut scratch[..output.len()];
                    if raw {
                        track.render_raw_audio(arrangement_frame, audio, pages.as_deref_mut())
                    } else if !source_mix.muted {
                        if let Some(page_provider) = pages.as_deref_mut() {
                            track.render_audio_additive_with_pages(
                                arrangement_frame,
                                audio,
                                page_provider,
                            )
                        } else {
                            track.render_audio_additive(arrangement_frame, audio)
                        }
                    } else {
                        Ok(())
                    }
                    .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                    fade.apply(arrangement_frame, audio);
                    for (target, sample) in output.iter_mut().zip(audio.iter()) {
                        target[0] += sample[0];
                        target[1] += sample[1];
                    }
                }
                if matches!(stage, SourceStage::Whole) && !source_mix.muted {
                    track
                        .render_instrument_additive_with_suppressions(
                            arrangement_frame,
                            performance_frame,
                            output,
                            suppressions,
                            continuations,
                            Some(revision.session().timing()),
                        )
                        .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                }
                if !matches!(stage, SourceStage::Instrument) {
                    return Ok(());
                }
            }
            if raw && matches!(stage, SourceStage::Audio) {
                track
                    .render_raw_audio(arrangement_frame, output, pages.as_deref_mut())
                    .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                return Ok(());
            }
            if raw && matches!(stage, SourceStage::Instrument) {
                return track
                    .render_raw_instrument(
                        arrangement_frame,
                        performance_frame,
                        output,
                        suppressions,
                        continuations,
                        Some(revision.session().timing()),
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
                            Some(revision.session().timing()),
                        )
                    } else if let Some(page_provider) = pages.as_deref_mut() {
                        track.render_additive_with_pages(
                            arrangement_frame,
                            output,
                            page_provider,
                            Some(revision.session().timing()),
                        )
                    } else {
                        // Arrangement volume uses the canonical clip clock,
                        // including the whole-track fast path. Buffered callers
                        // still own applying any track gain automation once.
                        track.render_additive_with_midi_floor(
                            arrangement_frame,
                            output,
                            None,
                            Some(revision.session().timing()),
                        )
                    }
                }
                SourceStage::Audio | SourceStage::Audition(_) => {
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
                    Some(revision.session().timing()),
                ),
            }
            .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
        }
        RouteOwner::Silence => {}
        RouteOwner::Session { clip, origin } => {
            if stage.is_audio() {
                return Ok(());
            }
            let prepared = midi
                .midi_clip_for_track(clip, route.id)
                .ok_or(SessionPlaybackError::UnknownClip)?;
            if prepared.track != route.id {
                return Err(SessionPlaybackError::ClipTrackMismatch);
            }
            if prepared.instrument_enabled() && !prepared.instrument_template().is_event_source() {
                let context =
                    crate::MidiInstrumentRenderContext::with_origin(origin, timing, table);
                let context = historical_floor.map_or(context, |floor| context.after_onset(floor));
                let context = context
                    .with_suppressions(suppressions)
                    .with_continuations(continuations);
                prepared
                    .instrument_template()
                    .render_additive_with_volume(
                        context,
                        source_mix,
                        performance_frame,
                        output,
                        spans,
                        prepared.volume_program(),
                    )
                    .map_err(SessionPlaybackError::Midi)?;
            }
        }
        RouteOwner::SessionAudio { clip, launch_frame } => {
            if matches!(stage, SourceStage::Instrument) {
                return Ok(());
            }
            render_session_audio_clip(
                revision,
                route.id,
                clip,
                launch_frame,
                performance_frame,
                output,
                source_mix,
                pages,
            )?;
        }
    }
    Ok(())
}

/// Adds one Session audio clip on its launch clock to `output`.
#[allow(clippy::too_many_arguments)]
fn render_session_audio_clip<P: MediaPages + ?Sized>(
    revision: &PreparedRevision,
    track: TrackId,
    clip: ClipId,
    launch_frame: u64,
    performance_frame: u64,
    output: &mut [Frame],
    source_mix: crate::PreparedTrackMix,
    pages: Option<&mut P>,
) -> Result<(), SessionPlaybackError> {
    let prepared = revision
        .session()
        .audio_clip(clip)
        .ok_or(SessionPlaybackError::UnknownClip)?;
    if prepared.track != track {
        return Err(SessionPlaybackError::ClipTrackMismatch);
    }
    if !prepared.active {
        return Ok(());
    }
    match prepared.source() {
        PreparedAudioSource::Resident(audio) => {
            render_resident_session_audio(
                prepared,
                audio,
                launch_frame,
                performance_frame,
                output,
                source_mix,
            )?;
        }
        PreparedAudioSource::Streamed(source) => {
            let page_provider = pages.ok_or(SessionPlaybackError::MediaPagesUnavailable)?;
            let Some(mut elapsed) = performance_frame.checked_sub(launch_frame) else {
                return Ok(());
            };
            let mut output_offset = 0_usize;
            // Every iteration renders at least one frame or stops. A
            // one-frame cycle is bounded by the admitted block length.
            while output_offset < output.len() {
                let Some(source_frame) = prepared.source_frame(elapsed) else {
                    break;
                };
                let remaining =
                    usize::try_from(prepared.playback_end().saturating_sub(source_frame))
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
                        reversed: false,
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
                output_offset = output_offset
                    .checked_add(
                        usize::try_from(rendered)
                            .map_err(|_| SessionPlaybackError::ArithmeticOverflow)?,
                    )
                    .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
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
    parameters_running: bool,
    output: &mut [Frame],
    mut capture: Option<&mut [Frame]>,
) -> Result<(), SessionPlaybackError> {
    for (offset, frame) in output.iter_mut().enumerate() {
        let relative =
            u64::try_from(offset).map_err(|_| SessionPlaybackError::ArithmeticOverflow)?;
        let absolute = release
            .sample_frame(performance_frame)?
            .checked_add(relative)
            .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
        let sample = if parameters_running && release.monitor_offset == 0 {
            release
                .voice
                .sample_at_with_continuation(absolute, contexts, retained, continuations)
        } else {
            release.voice.sample_at_with_clock(
                absolute,
                performance_frame
                    .checked_add(if parameters_running { relative } else { 0 })
                    .ok_or(SessionPlaybackError::ArithmeticOverflow)?,
                contexts,
                retained,
                continuations,
            )
        }
        .map_err(|_| SessionPlaybackError::SharedAnalyticContext)?;
        if let Some(sample) = sample {
            let sample = sample.map(|v| (f64::from(v) * release.clip_gain) as f32);
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
    parameters_running: bool,
    output: &mut [Frame],
) -> Result<(), SessionPlaybackError> {
    for (offset, frame) in output.iter_mut().enumerate() {
        let relative =
            u64::try_from(offset).map_err(|_| SessionPlaybackError::ArithmeticOverflow)?;
        let absolute = release
            .sample_frame(performance_frame)?
            .checked_add(relative)
            .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
        let sample = if parameters_running && release.monitor_offset == 0 {
            release
                .voice
                .sample_at_with_continuation(absolute, contexts, retained, continuations)
        } else {
            release.voice.sample_at_with_clock(
                absolute,
                performance_frame
                    .checked_add(if parameters_running { relative } else { 0 })
                    .ok_or(SessionPlaybackError::ArithmeticOverflow)?,
                contexts,
                retained,
                continuations,
            )
        }
        .map_err(|_| SessionPlaybackError::SharedAnalyticContext)?;
        if let Some(sample) = sample {
            frame[0] += (f64::from(sample[0]) * release.clip_gain) as f32;
            frame[1] += (f64::from(sample[1]) * release.clip_gain) as f32;
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
    fn tempo_micro_bpm_at_offset(&self, offset: usize) -> Option<u32> {
        self.timing
            .tempo_micro_bpm_at_frame(self.start.checked_add(offset as u64)?)
    }
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

#[cfg(test)]
mod native_input_transaction_tests {
    use super::*;
    use hex_lattice::{NativeTransientOperation, NativeTransientValue};
    use hex_lattice_contract::{DeviceAutomationTarget, NativeDeviceMetadata, ParameterValue};

    struct RefuseCapture(usize);
    impl CaptureSink for RefuseCapture {
        fn selects(&self, _: OutputCaptureId) -> bool {
            true
        }
        fn add(
            &mut self,
            _: OutputCaptureId,
            _: usize,
            _: &[Frame],
        ) -> Result<(), SessionPlaybackError> {
            self.0 += 1;
            Err(SessionPlaybackError::DeviceRuntime)
        }
    }

    #[test]
    fn native_input_late_capture_refusal_discards_application_and_retries_pristine_dsp() {
        let definition = &hex_devices::utility::UTILITY_DEFINITION;
        let state = definition
            .patch_parameter(
                &definition.default_state().unwrap(),
                hex_devices::utility::parameter::DC_REMOVE,
                ParameterValue::Bool(true),
            )
            .unwrap();
        let id = hex_devices::utility::parameter::GAIN;
        let target = DeviceAutomationTarget::Parameter(id);
        let (mut runtime, _) = super::super::native_parameter_input::tests::fixture_with_state(
            0,
            target,
            None,
            state.clone(),
        );
        let (mut pristine, _) =
            super::super::native_parameter_input::tests::fixture_with_state(0, target, None, state);
        let operation = NativeTransientOperation::Set(NativeTransientValue::Parameter {
            id,
            value: ParameterValue::Float(2.0),
        });
        runtime.set_native_parameter_input(operation).unwrap();
        pristine.set_native_parameter_input(operation).unwrap();
        let mut output = [[9.0; 2]; 16];
        let mut capture = RefuseCapture(0);
        assert!(
            runtime
                .render_internal::<dyn MediaPages>(
                    &mut output,
                    None,
                    None,
                    Some(&mut capture),
                    None
                )
                .is_err()
        );
        assert_eq!(capture.0, 1, "refuse after the actual target was processed");
        assert_eq!(output, [[0.0; 2]; 16]);
        assert_eq!(
            (runtime.performance_frame, runtime.arrangement_frame),
            (0, 0)
        );
        assert!(runtime.take_native_parameter_application().is_none());
        runtime.render(&mut output).unwrap();
        let mut expected = [[0.0; 2]; 16];
        pristine.render(&mut expected).unwrap();
        assert_eq!(output, expected);
        assert_ne!(
            output[0], output[15],
            "fixture must exercise retained filter history"
        );
        let applied = runtime.take_native_parameter_application().unwrap();
        assert_eq!(
            (applied.performance_frame, applied.arrangement_frame),
            (0, 0)
        );
    }
}
