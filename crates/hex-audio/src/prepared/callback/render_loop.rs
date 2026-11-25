//! Callback rendering entry points and audible position publication.
use crate::prepared::*;

impl Callback {
    pub fn render(&mut self, output: &mut [Frame]) -> Result<(), Invalid> {
        let block = self.scheduled_transport();
        let result = self.render_inner(output, block);
        self.release_current_quantum_pins();
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        if result.is_ok()
            && let Some((_, before, true)) = block
            && let Some((_, after, false)) = self.scheduled_transport()
            && let Ok(frame) = usize::try_from(after.saturating_sub(before))
            && frame < output.len()
            && self
                .active
                .as_ref()
                .and_then(ActivePublication::current)
                .is_some_and(|active| active.playback.device_insert_held())
        {
            self.native_sources.end_staging_at_rendered_prefix(frame);
        }
        if self.transport_fade.is_some() && !self.transport_fade_pending() {
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            if result.is_ok()
                && self.transport_fade_is_pause()
                && let Some((_, before, true)) = block
                && let Some((_, after, false)) = self.scheduled_transport()
                && let Ok(frame) = usize::try_from(after.saturating_sub(before))
                && frame < output.len()
            {
                self.native_sources.end_staging_at_rendered_prefix(frame);
            }
            self.settle_transport_fade();
        }
        if let Ok(outcome) = &result
            && !outcome.audible_recorded
        {
            self.record_audible_run(outcome.start, output.len());
        }
        if result.is_ok() {
            self.flush_session_route_transitions();
        }
        self.publish_audible_position();
        self.flush_stream_outcomes();
        result.map(|_| ())
    }

    pub(in crate::prepared) fn record_audible_run(
        &mut self,
        block: Option<(u64, u64, bool)>,
        frames: usize,
    ) {
        if let Some((position, performance, true)) = block
            && frames != 0
        {
            // A declicked Pause/Stop can end inside the block; only the frames
            // its performance clock advanced were played (#4151).
            let frames = match self.scheduled_transport() {
                Some((_, after, false)) => usize::try_from(after.saturating_sub(performance))
                    .map_or(frames, |played| played.min(frames)),
                _ => frames,
            };
            if frames > 0 {
                let region = self
                    .active
                    .as_ref()
                    .and_then(ActivePublication::current)
                    .and_then(|active| active.playback.audible_timing().1);
                self.audible_clock
                    .push(position, performance, frames, region);
            }
        }
    }

    /// Scheduled position, performance frame and playing state of the current graph.
    pub(in crate::prepared) fn scheduled_transport(&self) -> Option<(u64, u64, bool)> {
        self.active
            .as_ref()
            .and_then(ActivePublication::current)
            .map(|active| {
                (
                    active.playback.position(),
                    active.playback.performance_frame(),
                    active.playback.playing(),
                )
            })
    }

    pub(in crate::prepared) fn publish_audible_position(&self) {
        if let Some(active) = self.active.as_ref().and_then(ActivePublication::current) {
            let playback = &active.playback;
            let (latency, _) = playback.audible_timing();
            self.status.audible_position.store(
                self.audible_clock.audible(
                    playback.position(),
                    playback.performance_frame(),
                    latency,
                ),
                Relaxed,
            );
        }
    }
    /// Returns whether a split seek recorded its own suffix and the clock used
    /// for ordinary bookkeeping, adjusted for a pre-render streamed section.
    pub(in crate::prepared) fn render_inner(
        &mut self,
        output: &mut [Frame],
        initial: Option<(u64, u64, bool)>,
    ) -> Result<RenderOutcome, Invalid> {
        if self.status.device_fault.load(Acquire) {
            if !self.status.closing.load(Acquire) && !self.device_fault_fenced_in_block {
                self.fence_device_fault();
            }
            output.fill([0.0; 2]);
            return Ok(RenderOutcome {
                audible_recorded: false,
                start: initial,
            });
        }
        let yield_successor = self.yield_stream_successor_once;
        let scheduled_revision = self
            .deferred
            .as_ref()
            .and_then(|next| {
                matches!(
                    next.policy,
                    PublicationPolicy::InstallQuantizedSeekWhenReady { .. }
                        | PublicationPolicy::InheritAtTempoCut(_)
                )
                .then_some(next.revision)
            })
            .or_else(|| {
                self.deferred_eligible
                    .as_ref()
                    .filter(|next| next.tempo_cut().is_some())
                    .map(|next| next.revision())
            });
        if !output.is_empty() {
            self.yield_stream_successor_once = false;
        }
        if !yield_successor
            && !self.status.closing.load(Acquire)
            && self
                .deferred
                .as_ref()
                .is_some_and(|next| next.streams.is_some())
        {
            self.advance_legacy_publication(Some(output.len()));
        }

        if !yield_successor
            && !self.status.closing.load(Acquire)
            && self
                .deferred_eligible
                .as_ref()
                .is_some_and(|next| next.streaming_enabled())
        {
            self.advance_eligible_publication(Some(output.len()));
        }
        if !output.is_empty() {
            self.prepare_midi_recording_quantum(output.len());
        }
        // A replacement may clear the fault above. Until then, no failed graph
        // or its device chain is rendered, including while transport is stopped.
        // Take and live-input quanta still run so a racing take or gesture
        // receives its `OutputFailed` terminal instead of waiting on silence.
        if self.status.render_fault.load(Acquire) {
            self.reject_paused_monitors(input_monitor::MonitorStatus::OutputFault);
            self.sample_recording
                .end(crate::sample_recording::End::OutputFailed);
            self.rolling_recorder.discard_stage();
            if !output.is_empty() {
                self.prepare_gain_recording_quantum();
                self.prepare_native_parameter_quantum();
            }
            output.fill([0.0; 2]);
            return Ok(RenderOutcome {
                audible_recorded: false,
                start: initial,
            });
        }
        let rolling_generation = self
            .active
            .as_ref()
            .and_then(ActivePublication::current)
            .map(|active| active.control_context_generation);
        if self.output_transport.is_some() {
            self.reject_paused_monitors(input_monitor::MonitorStatus::Unsupported);
            self.sample_recording
                .end(crate::sample_recording::End::UnsupportedOutput);
            self.rolling_recorder.discard_stage();
        } else {
            self.sample_recording.begin(output.len());
            self.rolling_recorder
                .begin(output.len(), rolling_generation, self.rate);
        }
        if !yield_successor {
            self.prewarm_cold_device_fade(output.len());
        }
        // A ready streamed section can install above, before any frames
        // render. Only that transition replaces the original block start.
        let render_start = if scheduled_revision.is_some_and(|revision| {
            self.active
                .as_ref()
                .and_then(ActivePublication::current)
                .is_some_and(|active| active.revision == revision)
        }) {
            self.scheduled_transport()
        } else {
            initial
        };
        let scheduled = if self.status.closing.load(Acquire) {
            section_launch::ScheduledCutRender::NotHandled
        } else {
            self.render_scheduled_cut(output)?
        };
        if matches!(scheduled, section_launch::ScheduledCutRender::NotHandled) {
            self.render_project(output, 0)?;
        }
        if self.rolling_recorder.wants_main() {
            self.rolling_recorder.observe_main(output);
        }
        self.rolling_recorder
            .finish(!self.status.closing.load(Acquire) && !self.status.render_fault.load(Acquire));
        self.report_sampler_keyboard_rejection();
        if !self.status.closing.load(Acquire) {
            self.sample_preview.mix(output);
        }
        Ok(RenderOutcome {
            audible_recorded: matches!(scheduled, section_launch::ScheduledCutRender::Switched),
            start: render_start,
        })
    }

    pub(in crate::prepared) fn render_project(
        &mut self,
        output: &mut [Frame],
        capture_offset: usize,
    ) -> Result<(), Invalid> {
        if self.native_effects_revoked() {
            self.reject_paused_monitors(input_monitor::MonitorStatus::OutputFault);
            output.fill([0.0; 2]);
            return if self.status.playing.load(Relaxed) {
                Err(Invalid("native effect callback is revoked"))
            } else {
                Ok(())
            };
        }
        self.read_sampler_keyboard();
        if let Some(ActivePublication::Candidate(active)) = self.active.as_mut() {
            self.input_monitor
                .reject_paused(input_monitor::MonitorStatus::Unsupported);
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            self.paused_notes
                .reject_paused(input_monitor::MonitorStatus::Unsupported);
            if self.status.closing.load(Acquire) {
                output.fill([0.0; 2]);
                return Ok(());
            }
            #[cfg(all(test, feature = "allocation-probe"))]
            {
                active.owner.test_render(output)?;
                self.publish_transport();
                return Ok(());
            }
            #[cfg(not(all(test, feature = "allocation-probe")))]
            {
                let _ = active;
                return Err(Invalid("candidate admission is unavailable"));
            }
        }

        if !output.is_empty() {
            self.prepare_gain_recording_quantum();
            self.prepare_native_parameter_quantum();
        }
        self.reconcile_processor_audition();
        self.begin_project_spectrum(output.len());
        let active = self.active.as_ref().and_then(ActivePublication::current);
        let targets = self
            .spectrum_capture
            .as_ref()
            .and(self.spectrum_target.current_targets());
        self.spectrum_stages.begin(
            targets,
            self.spectrum_target.current_epoch(),
            output.len(),
            |target| {
                active.as_ref().is_some_and(|active| {
                    active.control_context_generation == target.project_generation
                        && active.revision >= target.activation_floor
                        && active.playback.sample_rate() == target.sample_rate_hz
                        && target.sample_rate_hz == self.rate
                })
            },
        );
        let node_observed = self.spectrum_stages.observing();
        let project_observed = self
            .spectrum_capture
            .as_ref()
            .and_then(|c| c.project.as_ref())
            .is_some_and(|p| p.observing());
        let recording_source = self.recording_source();
        if self.sample_recording.selection().is_some_and(|layout| {
            self.status.closing.load(Acquire)
                || layout
                    .expected_source
                    .is_some_and(|source| Some(source) != recording_source)
                || active.as_ref().is_none_or(|active| {
                    active.control_context_generation != layout.generation
                        || active.playback.sample_rate() != self.rate
                        || !active.playback.project_sample_sources_supported(layout)
                })
        }) {
            self.sample_recording
                .end(crate::sample_recording::End::SourceChanged);
        }
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        if self.input_monitor.has_paused_binding() {
            self.paused_notes
                .reject_paused(input_monitor::MonitorStatus::Unsupported);
        }
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        if self.paused_notes.has_paused_source_binding() {
            let playback = self
                .active
                .as_mut()
                .and_then(ActivePublication::legacy_mut)
                .filter(|graph| {
                    capture_offset == 0
                        && !self.status.closing.load(Acquire)
                        && self.output_transport.is_none()
                        && self.recovery_seek_pending.is_none()
                        && self.stream_pending_play.is_none()
                        && self.transport_fade.is_none()
                        && graph
                            .streams
                            .as_ref()
                            .is_none_or(|streams| streams.permits_paused_monitor())
                })
                .map(|graph| graph.playback.as_mut());
            let stages = &mut self.spectrum_stages;
            let mut observe = |identity: hex_engine::PostNodeIdentity,
                               phase: hex_engine::NodeSignalPhase,
                               offset: usize,
                               frames: &[Frame]| {
                stages.observe_signal(targets, identity, phase, offset, frames)
            };
            let observer: Option<&mut hex_engine::NodeSignalObserver<'_>> =
                node_observed.then_some(&mut observe);
            if self.paused_notes.render_paused_source(
                recording_source,
                self.status.transport_epoch.load(Acquire),
                playback,
                output,
                observer,
                self.paused_monitor_host_frames.map(|frames| {
                    native_effects::PausedNoteBanks::new(
                        &mut self.native_sources,
                        &mut self.paused_native_sources,
                        frames,
                    )
                }),
            ) {
                self.publish_transport();
                self.publish_active_routes(false);
                return Ok(());
            }
        }
        let active = self.active.as_ref().and_then(ActivePublication::current);
        let resident = self
            .active
            .as_ref()
            .is_some_and(|active| !active.streaming_enabled());
        let mut recording = self.sample_recording.observation();
        if self.input_monitor.has_paused_binding() {
            let paused_playback = self
                .active
                .as_ref()
                .and_then(ActivePublication::legacy)
                .filter(|graph| {
                    !node_observed
                        && !project_observed
                        && recording.is_none()
                        && !self.status.closing.load(Acquire)
                        && self.recovery_seek_pending.is_none()
                        && self.stream_pending_play.is_none()
                        && self.transport_fade.is_none()
                        && graph
                            .streams
                            .as_ref()
                            .is_none_or(|streams| streams.permits_paused_monitor())
                })
                .map(|active| active.playback.as_ref());
            if self.input_monitor.render_paused(
                recording_source,
                self.status.transport_epoch.load(Acquire),
                paused_playback,
                output,
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                self.paused_monitor_host_frames
                    .map(|frames| (&mut self.paused_native_sources, frames)),
            ) {
                self.publish_transport();
                self.publish_active_routes(false);
                return Ok(());
            }
        }
        let monitor_track = self.input_monitor.target();
        let monitor_supported = output.len() <= 256
            && resident
            && !project_observed
            && recording
                .as_ref()
                .is_none_or(|(layout, _)| layout.timing_only)
            && active.as_ref().is_some_and(|active| {
                monitor_track.is_some_and(|target| match target {
                    hex_engine::OutputCaptureId::Track(track) => {
                        active.playback.project_output_supported(&[track.value()])
                    }
                    _ => false,
                })
            });
        let playing = active
            .as_ref()
            .is_some_and(|active| active.playback.playing());
        let mut live_input = monitor_track.map(|_| [[0.0; 2]; 256]);
        let live_len = output.len().min(256);
        let monitor = live_input.as_mut().and_then(|input| {
            self.input_monitor.read(
                recording_source,
                monitor_supported,
                playing,
                &mut input[..live_len],
            )
        });
        let mut rolling_track = self.rolling_recorder.track_target();
        if let Some(track_id) = rolling_track
            && active
                .as_ref()
                .is_none_or(|active| !active.playback.project_output_supported(&[track_id]))
        {
            self.rolling_recorder.discard_stage();
            rolling_track = None;
        }
        let rolling_track_slice = rolling_track.map(|id| [id]);
        if !node_observed
            && !project_observed
            && recording.is_none()
            && monitor.is_none()
            && rolling_track.is_none()
        {
            return self.render_unobserved(output, capture_offset);
        }
        if self.status.closing.load(Acquire) {
            output.fill([0.0; 2]);
            return Ok(());
        }
        let stages = &mut self.spectrum_stages;
        let targets = self.spectrum_target.current_targets();
        let mut observe = |identity: hex_engine::PostNodeIdentity,
                           phase: hex_engine::NodeSignalPhase,
                           offset: usize,
                           frames: &[Frame]| {
            stages.observe_signal(targets, identity, phase, offset, frames)
        };
        let mut project_capture = self
            .spectrum_capture
            .as_mut()
            .and_then(|c| c.project.as_mut());
        let selection = project_capture
            .as_ref()
            .and_then(|p| p.selection())
            .filter(|_| project_observed);
        let (record_tracks, returns) = recording
            .as_ref()
            .map_or((&[][..], &[][..]), |(layout, _)| {
                (layout.tracks.as_ref(), layout.returns.as_ref())
            });
        let spectrum_tracks = selection.as_ref().map_or(&[][..], |s| s.tracks());
        let mut joined_base = [0; crate::sample_recording::TRACKS];
        let base_tracks =
            crate::sample_recording::merge_tracks(spectrum_tracks, record_tracks, &mut joined_base);
        let mut joined = [0; crate::sample_recording::TRACKS];
        let tracks = match (base_tracks, rolling_track_slice.as_ref()) {
            (Some(base), Some(rolling)) => {
                crate::sample_recording::merge_tracks(base, rolling, &mut joined)
            }
            (base, None) => base,
            (None, Some(_)) => None,
        };
        // A failed selection merge must invalidate recording even when its
        // timing-only layout no longer relies on a Main PCM observation.
        if project_observed
            && tracks.is_none()
            && let Some((layout, stage)) = recording.as_mut()
        {
            stage.observe(layout, hex_engine::ProjectSignal::Unavailable, 0, &[]);
        }
        let recording_observed = recording.is_some();
        let rolling_observed = rolling_track.is_some();
        let timing_only_recording = recording
            .as_ref()
            .is_some_and(|(layout, _)| layout.timing_only);
        let recording = std::cell::RefCell::new(recording);
        let rolling_recorder = &mut self.rolling_recorder;
        let mut observe_project = |signal, offset, frames: &[Frame]| {
            if let Some(project) = project_capture.as_mut()
                && match signal {
                    hex_engine::ProjectSignal::Track(id) => {
                        spectrum_tracks.binary_search(&id).is_ok()
                    }
                    hex_engine::ProjectSignal::Return(_) => false,
                    _ => true,
                }
            {
                project.observe(signal, offset, frames);
            }
            if recording_observed
                && let Ok(mut recording) = recording.try_borrow_mut()
                && let Some((layout, stage)) = recording.as_mut()
            {
                stage.observe(layout, signal, capture_offset + offset, frames);
            }
            if rolling_observed {
                rolling_recorder.observe_track(signal, capture_offset + offset, frames);
            }
        };
        let mut observe_timing = |offset, start, frames, map: &hex_engine::PreparedTempoMap| {
            if let Ok(mut recording) = recording.try_borrow_mut()
                && let Some((_, stage)) = recording.as_mut()
            {
                stage.observe_timing(capture_offset + offset, start, frames, map);
            }
        };
        let mut project = if timing_only_recording && !project_observed && !rolling_observed {
            Some(hex_engine::ProjectOutputTap::timing_only(
                &mut observe_timing,
                &mut observe_project,
            ))
        } else {
            tracks
                .filter(|_| project_observed || recording_observed || rolling_observed)
                .and_then(|tracks| {
                    let tap = hex_engine::ProjectOutputTap::with_returns(
                        tracks,
                        returns,
                        &mut observe_project,
                    )?;
                    Some(if recording_observed {
                        tap.with_timing(&mut observe_timing)
                    } else {
                        tap
                    })
                })
        };
        let node: Option<&mut hex_engine::NodeSignalObserver<'_>> = if node_observed {
            Some(&mut observe)
        } else {
            None
        };
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        let has_endpoints = self.native_sources.has_endpoints();
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        let mut foreign_window = NativeEffectWindow::new(&mut self.native_sources, capture_offset);
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        let foreign: Option<&mut dyn hex_lattice::ForeignProcessors> =
            has_endpoints.then_some(&mut foreign_window);
        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        let foreign = None;
        if let Some(mut active) = self
            .active
            .as_mut()
            .and_then(ActivePublication::stream_view)
        {
            let mut outcome = active.render_streamed_with_foreign(
                &mut self.domain_pages,
                self.stream_pending_play.is_some(),
                self.stream_prefix,
                output,
                node,
                project.as_mut(),
                foreign,
            );
            let action_pending = active.has_stream_action();
            let capacity_conflict = active.capacity_conflict();
            streaming::count_starvation(
                &self.status,
                self.stream_pending_play.is_some(),
                action_pending,
                &outcome,
            );
            self.resolve_recovery_seek(action_pending, outcome.action_rejection);
            if capacity_conflict && self.yield_pending_stream_successor() {
                outcome.readiness_refused = false;
            }
            self.end_native_staging_at_stream_hold(capture_offset, output.len(), &outcome);
            let streaming::RecordedStreamOutcome {
                applied_play,
                applied_action,
                rejected_action,
                fault,
            } = streaming::record_outcome(
                &self.status,
                &mut self.stream_pending_play,
                self.stream_prefix,
                action_pending,
                outcome,
            );
            if let Some((ticket, reason)) = rejected_action {
                self.stage_stream_outcome(ticket, action_outcomes::Outcome::Rejected(reason));
            }
            if let Some(ticket) = applied_action {
                self.stage_stream_outcome(ticket, action_outcomes::Outcome::Applied);
            }
            if let Some(ticket) = applied_play {
                self.stage_stream_outcome(ticket, action_outcomes::Outcome::Applied);
            }
            if let Some(error) = fault {
                return Err(error);
            }
        } else if let Some(mut active) = self
            .active
            .as_mut()
            .and_then(ActivePublication::current_mut)
        {
            if let (Some(track), Some(input)) = (monitor, live_input.as_ref()) {
                active.playback.render_with_live_track_audio(
                    output,
                    track,
                    &input[..live_len],
                    node,
                    project.as_mut(),
                    foreign,
                )?;
            } else {
                let wave = self
                    .wave_pool
                    .as_mut()
                    .map(|pool| pool as &mut dyn hex_engine::WaveExecutor);
                active.playback.render_with_signal_observers(
                    output,
                    node,
                    project.as_mut(),
                    foreign,
                    wave,
                )?;
            }
        } else {
            output.fill([0.0; 2]);
        }
        self.publish_transport();
        self.publish_active_routes(false);
        Ok(())
    }

    pub(in crate::prepared) fn render_unobserved(
        &mut self,
        output: &mut [Frame],
        capture_offset: usize,
    ) -> Result<(), Invalid> {
        if self.status.closing.load(Acquire) {
            output.fill([0.0; 2]);
            return Ok(());
        }
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        let has_endpoints = self.native_sources.has_endpoints();
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        let mut foreign_window = NativeEffectWindow::new(&mut self.native_sources, capture_offset);
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        let foreign: Option<&mut dyn hex_lattice::ForeignProcessors> =
            has_endpoints.then_some(&mut foreign_window);
        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        let foreign = {
            let _ = capture_offset;
            None
        };
        if let Some(mut active) = self
            .active
            .as_mut()
            .and_then(ActivePublication::stream_view)
        {
            let mut outcome = active.render_streamed_with_foreign(
                &mut self.domain_pages,
                self.stream_pending_play.is_some(),
                self.stream_prefix,
                output,
                None,
                None,
                foreign,
            );
            let action_pending = active.has_stream_action();
            let capacity_conflict = active.capacity_conflict();
            streaming::count_starvation(
                &self.status,
                self.stream_pending_play.is_some(),
                action_pending,
                &outcome,
            );
            self.resolve_recovery_seek(action_pending, outcome.action_rejection);
            if capacity_conflict && self.yield_pending_stream_successor() {
                outcome.readiness_refused = false;
            }
            self.end_native_staging_at_stream_hold(capture_offset, output.len(), &outcome);
            let streaming::RecordedStreamOutcome {
                applied_play,
                applied_action,
                rejected_action,
                fault,
            } = streaming::record_outcome(
                &self.status,
                &mut self.stream_pending_play,
                self.stream_prefix,
                action_pending,
                outcome,
            );
            self.publish_transport();
            self.publish_active_routes(false);
            if let Some((ticket, reason)) = rejected_action {
                self.stage_stream_outcome(ticket, action_outcomes::Outcome::Rejected(reason));
            }
            if let Some(ticket) = applied_action {
                self.stage_stream_outcome(ticket, action_outcomes::Outcome::Applied);
            }
            if let Some(ticket) = applied_play {
                self.stage_stream_outcome(ticket, action_outcomes::Outcome::Applied);
            }
            if let Some(error) = fault {
                return Err(error);
            }
            return Ok(());
        }
        if self.active.is_none()
            && (self
                .deferred
                .as_ref()
                .is_some_and(|next| next.streams.is_some())
                || self
                    .deferred_eligible
                    .as_ref()
                    .is_some_and(|next| next.streaming_enabled()))
        {
            output.fill([0.0; 2]);
            let fault = streaming::record_outcome(
                &self.status,
                &mut self.stream_pending_play,
                self.stream_prefix,
                false,
                crate::candidate_custody::StreamOutcome {
                    progress: crate::candidate_custody::StreamProgress::Buffering {
                        committed_frames: 0,
                    },
                    play_applied: false,
                    readiness_refused: false,
                    source_changed: false,
                    read_failed: false,
                    fault: None,
                    action_rejection: None,
                    action_applied: None,
                    activated: None,
                },
            )
            .fault;
            if let Some(error) = fault {
                return Err(error);
            }
            return Ok(());
        }
        if let Some(mut active) = self
            .active
            .as_mut()
            .and_then(ActivePublication::current_mut)
        {
            let wave = self
                .wave_pool
                .as_mut()
                .map(|pool| pool as &mut dyn hex_engine::WaveExecutor);
            active
                .playback
                .render_with_signal_observers(output, None, None, foreign, wave)?;
        } else {
            output.fill([0.0; 2]);
        }
        self.publish_transport();
        self.publish_active_routes(false);
        Ok(())
    }
}
