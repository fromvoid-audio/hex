mod compensated;
mod computer_keyboard;
mod detached_tails;
mod instrument_automation;
mod mutable_chunk;
mod mutable_setup;
mod playing;
mod return_buses;
mod route_chunk;
mod shared_source;
mod source_live;
#[cfg(test)]
mod wave_tests;
use super::capture::CaptureSink;
use super::return_effect_tails::MainTailClock;
use super::{commands::transition_captures, *};
use crate::compile::PreparedAudioSource;
use crate::{MediaPages, MidiTiming, StreamedPageSpan, render_streamed_page_span};
use detached_tails::process_mutable_tail;
use hex_project::LoopRegion;
pub(super) use mutable_chunk::collect_console_midi_onsets;
use mutable_chunk::{
    InstrumentClock, MutableInstrumentChunkContext, render_mutable_instrument_chunk,
};
pub(super) use route_chunk::normalize_loop_cursor;
use route_chunk::{
    RouteSourceContext, SourceStage, next_pending_frame, render_release_captured,
    render_release_raw, render_resident_session_audio, render_route_source_chunk,
};
use shared_source::{render_source_instrument_chunk, shared_source_has_route};
use std::sync::Arc;

/// Borrowed live PCM frames routed into a single track before its device chain.
#[derive(Clone, Copy)]
pub struct LiveTrackAudio<'a> {
    pub track: TrackId,
    pub frames: &'a [Frame],
}

#[derive(Clone, Copy)]
pub(crate) struct LiveAudioInputs<'a> {
    pub(crate) live_audio: LiveTrackAudio<'a>,
    pub(crate) sources: Option<crate::native_device_resources::SamplerSourcesRef<'a>>,
}

#[derive(Clone, Copy)]
pub(super) struct RenderBlockInputs<'a> {
    pub(super) sources: Option<crate::native_device_resources::SamplerSourcesRef<'a>>,
    pub(super) live_audio: Option<LiveTrackAudio<'a>>,
    pub(super) pending_monitor_pcm: bool,
}

pub(super) struct RenderSinks<'a, 'obs, 'obs_in, 'cap, 'wave> {
    pub(super) observer: &'a mut Option<&'obs mut crate::NodeSignalObserver<'obs_in>>,
    pub(super) capture: &'a mut Option<&'cap mut dyn CaptureSink>,
    pub(super) wave: &'a mut Option<&'wave mut dyn crate::wave_executor::WaveExecutor>,
}

#[derive(Clone, Copy)]
pub(super) struct RouteChunkTiming {
    pub(super) performance_frame: u64,
    pub(super) arrangement_frame: u64,
    pub(super) arrangement_loop_pass: u64,
    pub(super) frame_offset: usize,
}

pub(super) struct ForeignRenderStage<'a> {
    pub(super) foreign: &'a mut dyn hex_lattice::ForeignProcessors,
    pub(super) external: Option<&'a dyn hex_lattice::NativeExternalAutomationSet>,
}

pub(super) struct DetachedTailRouting<'a> {
    pub(super) routed: Option<&'a mut [Frame]>,
    pub(super) routed_sends: Option<&'a mut [Frame]>,
}

impl DetachedTailRouting<'_> {
    pub(super) const UNROUTED: Self = Self {
        routed: None,
        routed_sends: None,
    };
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

    /// Render a complete block transactionally using `wave` to execute
    /// independent topological waves of native processor chains in parallel.
    pub fn render_with_wave_executor(
        &mut self,
        output: &mut [Frame],
        wave: &mut dyn crate::wave_executor::WaveExecutor,
    ) -> Result<(), SessionPlaybackError> {
        self.render_with_signal_observers_and_wave(output, None, None, Some(wave))
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
    pub fn render_with_live_track_audio(
        &mut self,
        output: &mut [Frame],
        live_audio: LiveTrackAudio<'_>,
        pages: Option<&mut dyn MediaPages>,
        observer: Option<&mut crate::NodeSignalObserver<'_>>,
        project: Option<&mut ProjectOutputTap<'_, '_>>,
        foreign: &mut dyn hex_lattice::ForeignProcessors,
    ) -> Result<(), SessionPlaybackError> {
        self.render_live_audio(
            output,
            LiveAudioInputs {
                live_audio,
                sources: None,
            },
            pages,
            observer,
            project,
            foreign,
        )
    }

    pub(crate) fn render_live_audio<P: MediaPages + ?Sized>(
        &mut self,
        output: &mut [Frame],
        inputs: LiveAudioInputs<'_>,
        pages: Option<&mut P>,
        observer: Option<&mut crate::NodeSignalObserver<'_>>,
        mut project: Option<&mut ProjectOutputTap<'_, '_>>,
        foreign: &mut dyn hex_lattice::ForeignProcessors,
    ) -> Result<(), SessionPlaybackError> {
        let LiveAudioInputs {
            live_audio,
            sources,
        } = inputs;
        if self
            .validate_live_audio(
                live_audio.track,
                live_audio.frames,
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
            foreign,
            RenderBlockInputs {
                sources,
                live_audio: Some(live_audio),
                pending_monitor_pcm: false,
            },
            None,
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
        if self.playing || self.stopped_input.is_none_or(|scope| scope.track != track) {
            output.fill([0.0; 2]);
            return Err(SessionPlaybackError::InvalidCommand);
        }
        self.render_internal_with_live_audio::<dyn MediaPages>(
            output,
            None,
            None,
            None,
            foreign,
            RenderBlockInputs {
                sources: None,
                live_audio: Some(LiveTrackAudio {
                    track,
                    frames: input,
                }),
                pending_monitor_pcm: false,
            },
            None,
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
        project: Option<&mut ProjectOutputTap<'_, '_>>,
        foreign: &mut dyn hex_lattice::ForeignProcessors,
    ) -> Result<(), SessionPlaybackError> {
        self.render_with_foreign_processors_and_wave(
            output, pages, observer, project, foreign, None,
        )
    }

    /// Foreign-processor render entry that also executes eligible independent
    /// native waves through `wave`.
    #[allow(clippy::too_many_arguments)]
    pub fn render_with_foreign_processors_and_wave(
        &mut self,
        output: &mut [Frame],
        pages: Option<&mut dyn MediaPages>,
        observer: Option<&mut crate::NodeSignalObserver<'_>>,
        mut project: Option<&mut ProjectOutputTap<'_, '_>>,
        foreign: &mut dyn hex_lattice::ForeignProcessors,
        wave: Option<&mut dyn crate::wave_executor::WaveExecutor>,
    ) -> Result<(), SessionPlaybackError> {
        self.candidate_admission_open = false;
        let capture = project
            .as_mut()
            .map(|tap| &mut **tap as &mut dyn CaptureSink);
        self.render_internal_with_foreign_and_wave(
            output, pages, observer, capture, None, foreign, wave,
        )?;
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
            let mut adapter = crate::PostNodeSignalAdapter(observer);
            self.render_with_node_signal_observer(output, Some(&mut adapter))
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
        project: Option<&mut ProjectOutputTap<'_, '_>>,
    ) -> Result<(), SessionPlaybackError> {
        self.render_with_signal_observers_and_wave(output, observer, project, None)
    }

    /// Signal-observer render entry that also executes eligible independent
    /// native waves through `wave`.
    pub fn render_with_signal_observers_and_wave(
        &mut self,
        output: &mut [Frame],
        observer: Option<&mut crate::NodeSignalObserver<'_>>,
        mut project: Option<&mut ProjectOutputTap<'_, '_>>,
        wave: Option<&mut dyn crate::wave_executor::WaveExecutor>,
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
        self.render_internal_with_foreign_and_wave::<dyn MediaPages>(
            output,
            None,
            observer,
            capture,
            None,
            &mut hex_lattice::RefuseForeign,
            wave,
        )?;
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
            let mut adapter = crate::PostNodeSignalAdapter(observer);
            self.render_with_pages_and_node_signal_observer(output, pages, Some(&mut adapter))
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
        self.render_internal_with_foreign_and_wave(
            output, pages, observer, capture, sources, foreign, None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn render_internal_with_foreign_and_wave<P: MediaPages + ?Sized>(
        &mut self,
        output: &mut [Frame],
        pages: Option<&mut P>,
        observer: Option<&mut crate::NodeSignalObserver<'_>>,
        capture: Option<&mut dyn CaptureSink>,
        sources: Option<crate::native_device_resources::SamplerSourcesRef<'_>>,
        foreign: &mut dyn hex_lattice::ForeignProcessors,
        wave: Option<&mut dyn crate::wave_executor::WaveExecutor>,
    ) -> Result<(), SessionPlaybackError> {
        self.render_internal_with_live_audio(
            output,
            pages,
            observer,
            capture,
            foreign,
            RenderBlockInputs {
                sources,
                live_audio: None,
                pending_monitor_pcm: false,
            },
            wave,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn render_internal_with_live_audio<P: MediaPages + ?Sized>(
        &mut self,
        output: &mut [Frame],
        pages: Option<&mut P>,
        observer: Option<&mut crate::NodeSignalObserver<'_>>,
        capture: Option<&mut dyn CaptureSink>,
        foreign: &mut dyn hex_lattice::ForeignProcessors,
        inputs: RenderBlockInputs<'_>,
        wave: Option<&mut dyn crate::wave_executor::WaveExecutor>,
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
                    foreign,
                    inputs,
                    wave,
                )
            }
            None => self.render_internal_metered::<P>(
                output, None, observer, capture, foreign, inputs, wave,
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
        foreign: &mut dyn hex_lattice::ForeignProcessors,
        inputs: RenderBlockInputs<'_>,
        wave: Option<&mut dyn crate::wave_executor::WaveExecutor>,
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
                foreign,
                inputs,
                wave,
            );
            if result.is_err() {
                meter.invalidate();
            }
            self.meter_cells = cells;
            result
        } else {
            self.render_on_active_route(output, pages, observer, capture, foreign, inputs, wave)
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
            foreign,
            RenderBlockInputs {
                sources: None,
                live_audio: None,
                pending_monitor_pcm,
            },
            None,
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
        foreign: &mut dyn hex_lattice::ForeignProcessors,
        inputs: RenderBlockInputs<'_>,
        wave: Option<&mut dyn crate::wave_executor::WaveExecutor>,
    ) -> Result<(), SessionPlaybackError> {
        if self.defer_paused_source_monitor
            && !self.playing
            && (self.performance_frame != 0 || self.arrangement_frame != 0)
            && inputs.live_audio.is_none()
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
            if inputs.live_audio.is_none() && self.permits_resident_pause_source_monitor() {
                let result = monitor.render_resident(self, output, observer, capture, foreign);
                self.paused_source_monitor = Some(monitor);
                return result;
            }
            monitor.end_resident_interval();
            self.paused_source_monitor = Some(monitor);
        }
        self.render_internal_borrowed(output, pages, observer, capture, foreign, inputs, wave)
    }

    #[allow(clippy::too_many_arguments)]
    fn render_internal_borrowed<P: MediaPages + ?Sized>(
        &mut self,
        output: &mut [Frame],
        pages: Option<&mut P>,
        observer: Option<&mut crate::NodeSignalObserver<'_>>,
        mut capture: Option<&mut dyn CaptureSink>,
        foreign: &mut dyn hex_lattice::ForeignProcessors,
        inputs: RenderBlockInputs<'_>,
        mut wave: Option<&mut dyn crate::wave_executor::WaveExecutor>,
    ) -> Result<(), SessionPlaybackError> {
        let RenderBlockInputs {
            sources,
            live_audio,
            pending_monitor_pcm,
        } = inputs;
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
                    .all(|tail| tail.committed_is_quiescent()))
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
            if tail.is_powering_off() {
                continue;
            }
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
        self.return_fade_scratch = self.return_fade;
        self.live_gain_scratch = self.live_gain;
        let result = if self.playing {
            self.render_playing(
                output,
                pages,
                &mut observer,
                &mut capture,
                &mut wave,
                foreign,
                inputs,
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
                tail.rollback_block();
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
            self.return_fade = self.return_fade_scratch.filter(|fade| fade.fader.active());
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
