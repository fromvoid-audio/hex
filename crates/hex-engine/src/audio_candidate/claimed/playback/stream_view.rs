//! Borrowed stream operations on an existing musical runtime. This facade owns
//! no graph, resource, source, endpoint or readiness authority. Ordinary callers
//! supply correlation metadata from their publication; constructor callers retain
//! their original phase checks and closed-class restrictions. Nothing extracts
//! a Playback from constructor custody or adopts one into it.
use super::*;
use crate::{
    AudioSourceContext, Invalid, PlaybackAction, PlaybackActionError, PlaybackHandoffError,
    PreparedStreamAction, StreamChunkWindow, StreamResourceExtent,
};

enum Runtime<'a> {
    Constructor(&'a mut ClaimedAudioCandidateRuntime),
    Ordinary(&'a mut Playback, AudioSourceContext),
}
/// Stack-only operations shared by native page-readiness consumers. Correlation
/// is not authorization: the host must separately retain actual resource/domain
/// custody and exact protected pages before any render or publication commit.
pub struct StreamPlayback<'a> {
    runtime: Runtime<'a>,
}
impl ClaimedAudioCandidateRuntime {
    pub fn stream_playback(&mut self) -> StreamPlayback<'_> {
        StreamPlayback {
            runtime: Runtime::Constructor(self),
        }
    }
}
impl Playback {
    pub fn stream_playback(&mut self, context: AudioSourceContext) -> StreamPlayback<'_> {
        StreamPlayback {
            runtime: Runtime::Ordinary(self, context),
        }
    }
}
impl StreamPlayback<'_> {
    /// Ordinary paused monitoring still runs its existing keyboard/DSP kernel,
    /// with no clip-page demand or transport-clock advance. Constructor runtimes
    /// keep their closed-class paused behavior and never gain ordinary custody.
    pub fn renders_when_paused(&self) -> bool {
        matches!(self.runtime, Runtime::Ordinary(..))
    }

    pub fn source_context(&self) -> Option<AudioSourceContext> {
        match &self.runtime {
            Runtime::Constructor(r) => r.source_context(),
            Runtime::Ordinary(_, context) => Some(*context),
        }
    }
    pub fn sample_rate(&self) -> Option<u32> {
        match &self.runtime {
            Runtime::Constructor(r) => r.sample_rate(),
            Runtime::Ordinary(r, _) => Some(r.sample_rate()),
        }
    }
    pub fn status(&self) -> Option<AudioCandidateRuntimeStatus> {
        match &self.runtime {
            Runtime::Constructor(r) => r.status(),
            Runtime::Ordinary(r, _) => Some(AudioCandidateRuntimeStatus {
                position: r.position(),
                performance_frame: r.performance_frame(),
                playing: r.playing(),
                metronome_enabled: r.metronome_enabled(),
                session_capability: r.session_capability_status(),
                sampler_console: false,
                route_state_generation: r.session_route_state_generation(),
                route_count: r.session_route_count(),
                main_output_latency_frames: r.main_output_latency_frames(),
                content_loop: r.automation_capture_loop(),
            }),
        }
    }
    pub fn session_route_status(&self, index: usize) -> Option<crate::SessionRouteStatus> {
        match &self.runtime {
            Runtime::Constructor(r) => r.session_route_status(index),
            Runtime::Ordinary(r, _) => r.session_route_status(index),
        }
    }
    pub fn apply_action(&mut self, action: PlaybackAction) -> Result<(), PlaybackActionError> {
        match &mut self.runtime {
            Runtime::Constructor(r) => r.apply_action(action),
            Runtime::Ordinary(r, _) => r.try_apply_action(action),
        }
    }
    /// `apply_action` with the transport declick, so a ready streamed Play
    /// from a paused position ramps in like the resident path.
    pub fn apply_declicked_action(
        &mut self,
        action: PlaybackAction,
    ) -> Result<(), PlaybackActionError> {
        match &mut self.runtime {
            Runtime::Constructor(r) => r.apply_declicked_action(action),
            Runtime::Ordinary(r, _) => r.try_apply_declicked_action(action),
        }
    }
    pub fn prepare_stream_action(
        &mut self,
        action: PlaybackAction,
    ) -> Result<PreparedStreamAction, PlaybackActionError> {
        match &mut self.runtime {
            Runtime::Constructor(r) => r.prepare_stream_action(action),
            Runtime::Ordinary(r, context) => {
                let mut prepared = r.prepare_ordinary_stream_action(action)?;
                prepared.bind_ordinary_context(*context);
                Ok(prepared)
            }
        }
    }
    pub fn stream_action_targets_track(
        &mut self,
        prepared: &PreparedStreamAction,
        track: Option<hex_project::TrackId>,
    ) -> Result<bool, PlaybackActionError> {
        match &mut self.runtime {
            Runtime::Constructor(r) => r.stream_action_targets_track(prepared, track),
            Runtime::Ordinary(r, context) => {
                prepared.check_ordinary_context(*context)?;
                r.stream_action_targets_track(prepared, track)
            }
        }
    }
    pub fn commit_stream_action(
        &mut self,
        prepared: PreparedStreamAction,
    ) -> Result<(), PlaybackActionError> {
        match &mut self.runtime {
            Runtime::Constructor(r) => r.commit_stream_action(prepared),
            Runtime::Ordinary(r, context) => {
                prepared.check_ordinary_context(*context)?;
                r.commit_stream_action(prepared)
            }
        }
    }
    pub fn visit_prepared_stream_action(
        &mut self,
        prepared: &PreparedStreamAction,
        maximum_frames: usize,
        visit: &mut impl FnMut(StreamResourceExtent) -> Result<(), Invalid>,
    ) -> Result<StreamChunkWindow, Invalid> {
        match &mut self.runtime {
            Runtime::Constructor(r) => {
                r.visit_prepared_stream_action(prepared, maximum_frames, visit)
            }
            Runtime::Ordinary(r, context) => {
                prepared
                    .check_ordinary_context(*context)
                    .map_err(|_| Invalid("stream action context differs"))?;
                r.visit_prepared_stream_action(prepared, maximum_frames, visit)
            }
        }
    }
    pub fn visit_pending_stream_chunk(
        &mut self,
        due: u64,
        maximum_frames: usize,
        visit: &mut impl FnMut(StreamResourceExtent) -> Result<(), Invalid>,
    ) -> Result<StreamChunkWindow, Invalid> {
        match &mut self.runtime {
            Runtime::Constructor(r) => r.visit_pending_stream_chunk(due, maximum_frames, visit),
            Runtime::Ordinary(r, _) => {
                r.visit_ordinary_pending_stream_chunk(due, maximum_frames, visit)
            }
        }
    }
    pub fn visit_next_stream_chunk(
        &self,
        maximum_frames: usize,
        playing: Option<bool>,
        visit: &mut impl FnMut(StreamResourceExtent) -> Result<(), Invalid>,
    ) -> Result<StreamChunkWindow, Invalid> {
        match &self.runtime {
            Runtime::Constructor(r) => r.visit_next_stream_chunk(maximum_frames, playing, visit),
            Runtime::Ordinary(r, _) => {
                r.visit_ordinary_stream_chunk(maximum_frames, playing, visit)
            }
        }
    }
    /// Visit stream demand for at most 256 upcoming frames and return the
    /// covered width. A longer callback is only partially covered. This is a
    /// speculative observation, not a readiness receipt or transport commit.
    pub fn visit_current_stream_quantum(
        &mut self,
        maximum_frames: usize,
        playing: Option<bool>,
        visit: &mut impl FnMut(StreamResourceExtent) -> Result<(), Invalid>,
    ) -> Result<usize, Invalid> {
        match &mut self.runtime {
            Runtime::Constructor(r) => {
                r.visit_current_stream_quantum(maximum_frames, playing, visit)
            }
            Runtime::Ordinary(r, _) => {
                r.visit_current_stream_quantum(maximum_frames, playing, true, visit)
            }
        }
    }
    /// Visit at most 256 frames at an offset from the current clock. This does
    /// not advance playback or establish readiness for the visited resources.
    pub fn visit_current_stream_quantum_at(
        &mut self,
        start_offset: u64,
        maximum_frames: usize,
        playing: Option<bool>,
        visit: &mut impl FnMut(StreamResourceExtent) -> Result<(), Invalid>,
    ) -> Result<usize, crate::StreamQuantumProjectionError> {
        match &mut self.runtime {
            Runtime::Constructor(r) => {
                r.visit_current_stream_quantum_at(start_offset, maximum_frames, playing, visit)
            }
            Runtime::Ordinary(r, _) => r.visit_current_stream_quantum_at(
                start_offset,
                maximum_frames,
                playing,
                true,
                visit,
            ),
        }
    }
    pub fn preview_stream_handoff(
        &mut self,
        previous: &mut StreamPlayback<'_>,
        maximum_frames: usize,
        playing: Option<bool>,
        visit: &mut impl FnMut(StreamResourceExtent) -> Result<(), Invalid>,
    ) -> Result<StreamChunkWindow, PlaybackHandoffError> {
        self.preview_stream_handoff_with_report(previous, maximum_frames, playing, visit)
            .map(|(window, _)| window)
    }
    /// Preview stream demand and the running-audio handoff classification.
    /// The report does not authorize admission or a later publication.
    pub fn preview_stream_handoff_with_report(
        &mut self,
        previous: &mut StreamPlayback<'_>,
        maximum_frames: usize,
        playing: Option<bool>,
        visit: &mut impl FnMut(StreamResourceExtent) -> Result<(), Invalid>,
    ) -> Result<(StreamChunkWindow, crate::SessionHandoffReport), PlaybackHandoffError> {
        // Constructor/constructor retains its exact existing identity and class
        // preflight. Mixed ownership can only borrow closed Current runtimes;
        // it never changes their owner, anchors, phase or publication eligibility.
        if let (Runtime::Constructor(next), Runtime::Constructor(old)) =
            (&mut self.runtime, &mut previous.runtime)
        {
            return next.preview_stream_handoff_with_report(old, maximum_frames, playing, visit);
        }
        fn musical_runtime<'a>(
            runtime: &'a mut Runtime<'_>,
        ) -> Result<&'a mut Playback, PlaybackHandoffError> {
            match runtime {
                Runtime::Ordinary(r, _) => Ok(r),
                Runtime::Constructor(r) => {
                    if r.inner
                        .runtime
                        .as_ref()
                        .is_some_and(|r| r.sampler_sources.is_some())
                    {
                        return Err(PlaybackHandoffError::Session(
                            crate::SessionPlaybackError::InvalidCommand,
                        ));
                    }
                    r.current_playback_mut()
                        .ok_or(PlaybackHandoffError::Session(
                            crate::SessionPlaybackError::InvalidCommand,
                        ))
                }
            }
        }
        let ordinary = matches!(self.runtime, Runtime::Ordinary(..));
        let next = musical_runtime(&mut self.runtime)?;
        let old = musical_runtime(&mut previous.runtime)?;
        next.preview_stream_handoff_with_report(
            old,
            maximum_frames,
            playing,
            visit,
            ordinary,
            false,
        )
    }
    /// Preview only: the publication owner must validate the actual live fence
    /// and protect these pages before committing the corresponding reset.
    pub fn preview_ordinary_fenced_restart(
        &mut self,
        previous: &mut StreamPlayback<'_>,
        maximum_frames: usize,
        playing: Option<bool>,
        visit: &mut impl FnMut(StreamResourceExtent) -> Result<(), Invalid>,
    ) -> Result<StreamChunkWindow, PlaybackHandoffError> {
        self.preview_ordinary_fenced_restart_with_report(previous, maximum_frames, playing, visit)
            .map(|(window, _)| window)
    }
    /// Classify a fenced ordinary restart without committing its fence or pages.
    pub fn preview_ordinary_fenced_restart_with_report(
        &mut self,
        previous: &mut StreamPlayback<'_>,
        maximum_frames: usize,
        playing: Option<bool>,
        visit: &mut impl FnMut(StreamResourceExtent) -> Result<(), Invalid>,
    ) -> Result<(StreamChunkWindow, crate::SessionHandoffReport), PlaybackHandoffError> {
        match (&mut self.runtime, &mut previous.runtime) {
            (Runtime::Ordinary(next, _), Runtime::Ordinary(old, _)) => next
                .preview_stream_handoff_with_report(
                    old,
                    maximum_frames,
                    playing,
                    visit,
                    true,
                    true,
                ),
            _ => Err(PlaybackHandoffError::RuntimeFenceChanged),
        }
    }
    /// Borrow the existing admitted foreign endpoints through the same runtime
    /// facade. Neither this view nor page facts grant endpoint authority.
    pub fn render_with_foreign_processors(
        &mut self,
        output: &mut [crate::Frame],
        pages: Option<&mut dyn crate::MediaPages>,
        observer: Option<&mut crate::NodeSignalObserver<'_>>,
        project: Option<&mut crate::ProjectOutputTap<'_, '_>>,
        foreign: &mut dyn hex_lattice::ForeignProcessors,
    ) -> Result<(), Invalid> {
        match &mut self.runtime {
            Runtime::Constructor(runtime) => {
                runtime.render_with_foreign_processors(output, pages, observer, project, foreign)
            }
            Runtime::Ordinary(runtime, _) => {
                runtime.render_with_foreign_processors(output, pages, observer, project, foreign)
            }
        }
    }
    pub fn render_with_pages_and_signal_observers<P: crate::MediaPages + ?Sized>(
        &mut self,
        output: &mut [crate::Frame],
        pages: &mut P,
        observer: Option<&mut crate::NodeSignalObserver<'_>>,
        project: Option<&mut crate::ProjectOutputTap<'_, '_>>,
    ) -> Result<(), Invalid> {
        match &mut self.runtime {
            Runtime::Constructor(r) => {
                r.render_with_pages_and_signal_observers(output, pages, observer, project)
            }
            Runtime::Ordinary(r, _) => {
                r.render_with_pages_and_signal_observers(output, pages, observer, project)
            }
        }
    }
}
