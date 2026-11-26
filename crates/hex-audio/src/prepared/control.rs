//! Control-side publication, transport command queueing, and storage quotes.
use super::*;

/// Classification of a refused ordinary graph publication.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublishErrorKind {
    /// An original native source was unavailable during reference admission.
    /// The reason for its unavailability is not implied by this classification.
    NativeSourceUnavailable,
    /// Any other publication refusal; consult the diagnostic reason.
    Other,
}

/// Failed publication retains the candidate so the caller can retry or dispose
/// of it off the callback. Revisions advance only after successful publication.
pub struct PublishError {
    pub kind: PublishErrorKind,
    pub reason: &'static str,
    pub playback: Playback,
}

/// A failed full control-union admission retains the same unpublished graph, independently of
/// a queue/resource refusal. Neither outcome changes canonical history or acknowledges playback.
pub enum AdmittedPublishError<E> {
    Admission { reason: E, playback: Playback },
    Publication(PublishError),
}
impl<E: std::fmt::Debug> std::fmt::Debug for AdmittedPublishError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Admission { reason, .. } => f.debug_tuple("Admission").field(reason).finish(),
            Self::Publication(error) => std::fmt::Debug::fmt(error, f),
        }
    }
}

impl std::fmt::Debug for PublishError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PublishError")
            .field("kind", &self.kind)
            .field("reason", &self.reason)
            .finish_non_exhaustive()
    }
}

/// Legacy unsplit control. Native worker composition consumes this exactly once
/// before delivering its nonowning client. Resident callers retain these APIs.
pub struct Control {
    pub(super) diagnostics: audio_diagnostics::TraceReader,
    pub(super) action_admission: Option<action_outcomes::Admission>,
    pub sample_preview: crate::sample_preview::Control,
    pub sample_recording: crate::sample_recording::Control,
    pub rolling_recorder: crate::rolling_recorder::Control,
    pub input_monitor: input_monitor::Control,
    pub(super) commands: Producer<QueuedCommand>,
    pub spectrum_target: spectrum_target::SpectrumTargetControl,
    pub processor_audition: processor_audition::ProcessorAuditionTargetControl,
    pub gain_recording: crate::gain_recording::Control,
    pub native_parameter_input: crate::native_parameter_input::Control,
    pub midi_recording: crate::midi_recording::Control,
    pub(super) route_transitions: route_transitions::RouteTransitionControl,
    pub(super) command_ticket: u64,
    pub(super) publication: PublicationControl,
}
/// The unique worker-side publication/return endpoint; never a UI command client.
pub struct PublicationControl {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) paused_notes: input_monitor::Control,
    #[cfg_attr(
        not(all(target_os = "macos", target_arch = "aarch64")),
        allow(dead_code)
    )]
    pub(super) foreign_parameter_delivery: Option<crate::foreign_parameter_delivery::Control>,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) foreign_editor_delivery: Option<crate::foreign_parameter_delivery::editor::Control>,
    pub(super) plugin_callback_quantum: Option<u16>,
    pub(super) output_transport_reaper: Option<crate::plugin_worker::Reaper>,
    pub(super) publications: Producer<QueuedPublication>,
    pub(super) retired: Option<Consumer<ReturnedPublication>>,
    pub(super) candidate_root: crate::candidate_custody::CandidateRoot,
    pub(super) native_custody: bool,
    pub(super) held_control_return: Option<ReturnedPublication>,
    pub status: Arc<Status>,
    pub(super) rate: u32,
    pub(super) revision: u64,
    pub(super) command_storage_bytes: usize,
    pub(super) legacy_pcm_frames: u64,
    pub(super) device_resources:
        Option<hex_engine::native_device_resources::DeviceResourcePublication>,
    pub(super) resident_forwarding: Option<Arc<resident_forwarding::ResidentForwarding>>,
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(super) endpoint_probe: Option<crate::native_callback_owner::DropProbe>,
    pub(super) latest_fresh_runtime_cancellation: Option<CancellablePublicationReceipt>,
}
impl PublicationControl {
    /// Publish one private note monitor for the exact settled pause. The
    /// caller retains its storage hold and the corresponding native sources
    /// until this worker reclaims the monitor. Refusal returns the same owner.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub fn publish_paused_note_monitor(
        &mut self,
        monitor: hex_engine::PausedSourceMonitor,
        source: AppliedAudioSource,
        transport_epoch: u64,
    ) -> Result<(), hex_engine::PausedSourceMonitor> {
        if !self.native_custody
            || self.status.paused_note_source() != Some((source, transport_epoch))
        {
            return Err(monitor);
        }
        self.paused_notes
            .publish_paused_source(monitor, source, transport_epoch)
    }

    /// Observe the worker's last note monitor activation or termination.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub fn paused_note_monitor_status(&self) -> input_monitor::MonitorStatus {
        self.paused_notes.status()
    }

    /// Configure optional tracing on control. Default is off; old records may
    /// still be drained after disabling. The music/fault queues are independent.
    pub fn set_audio_diagnostics(&self, flags: AudioDiagnosticsFlags) {
        self.status.diagnostics_flags.store(flags.bits(), Relaxed);
    }
}
impl Control {
    /// Opt in on the original vacant constructor before callback or client
    /// exposure. Admission has 32 credits until the reader consumes outcomes;
    /// priority Stop uses its separate path. Streamed actions retain their
    /// credit through preparation; scheduled launches complete at scheduling,
    /// while activation remains a separate status. Admission may also refuse
    /// when the next ticket's slot is still occupied; consume
    /// available outcomes and retry. After shutdown, retain the reader until
    /// native retirement or retire the unsplit constructor on control.
    pub fn enable_action_outcomes_on_control(
        &mut self,
        callback: &mut Callback,
    ) -> Result<ActionOutcomeReader, &'static str> {
        if self.action_admission.is_some()
            || callback.action_writer.is_some()
            || self.command_ticket != 0
            || self.publication.revision != 0
            || self.publication.native_custody
            || self.status.split_endpoints.load(Acquire)
            || self.status.closing.load(Acquire)
            || !callback.transport_setup_open
            || !callback.matches_native_status(&self.status)
            || !callback.belongs_to_candidate_root(&self.publication.candidate_root)
        {
            return Err("action outcomes require a vacant paired constructor");
        }
        let (admission, writer, reader) =
            action_outcomes::channel(self.publication.candidate_root.root_id());
        self.action_admission = Some(admission);
        callback.action_writer = Some(writer);
        Ok(reader)
    }

    /// Consume an unsplit, nonnative pair after shutdown, when exclusive access
    /// to the callback establishes that it cannot execute concurrently.
    pub fn retire_action_outcomes_on_control(
        &mut self,
        callback: &mut Callback,
    ) -> Result<(), &'static str> {
        if !self.status.closing.load(Acquire)
            || self.publication.native_custody
            || self.status.split_endpoints.load(Acquire)
            || !callback.matches_native_status(&self.status)
            || !callback.belongs_to_candidate_root(&self.publication.candidate_root)
        {
            return Err("action outcomes require a quiescent paired constructor");
        }
        let Some(admission) = self.action_admission.take() else {
            return Err("action outcome admission unavailable");
        };
        let Some(writer) = callback.action_writer.take() else {
            self.action_admission = Some(admission);
            return Err("action outcome writer unavailable");
        };
        match writer.interrupt_after_quiescence(admission) {
            Ok(()) => Ok(()),
            Err((writer, admission)) => {
                callback.action_writer = Some(writer);
                self.action_admission = Some(admission);
                Err("action outcome endpoints do not match")
            }
        }
    }

    /// Take one non-owning Copy event; callers bound each drain.
    pub fn take_audio_diagnostic(&mut self) -> Option<AudioDiagnosticEvent> {
        self.diagnostics.pop()
    }
}

impl std::ops::Deref for Control {
    type Target = PublicationControl;
    fn deref(&self) -> &Self::Target {
        &self.publication
    }
}
impl std::ops::DerefMut for Control {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.publication
    }
}

/// Final worker-input disposition, distinct from host delivery. Cancellation
/// must replace an already copied host chunk with silence, never dispatch it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OutputTransportDelivery {
    Accepted,
    Cancelled,
    Refused,
}

pub(super) fn capture_runtime_fence(status: &Status) -> Option<FreshRuntimeFence> {
    let consumed_stop_generation = status.consumed_stop_generation.load(Acquire);
    if consumed_stop_generation & 1 != 0
        || status.pending_stop_marker.load(Acquire) != 0
        || status.render_fault.load(Acquire)
        || status.device_fault.load(Acquire)
    {
        return None;
    }
    let output_fault_epoch = status.last_output_fault_epoch.load(Acquire);
    let mut routes = [SessionRouteEntry::default(); SESSION_ROUTE_CAPACITY];
    let snapshot = status.session_routes(&mut routes)?;
    if status.pending_stop_marker.load(Acquire) != 0
        || status.consumed_stop_generation.load(Acquire) != consumed_stop_generation
        || status.render_fault.load(Acquire)
        || status.device_fault.load(Acquire)
        || status.last_output_fault_epoch.load(Acquire) != output_fault_epoch
    {
        return None;
    }
    Some(FreshRuntimeFence {
        expected_active_revision: snapshot.audio_revision,
        route_state_generation: snapshot.route_state_generation,
        admitted_command_ticket: status.admitted_ticket.load(Acquire),
        output_fault_epoch,
        consumed_stop_generation,
    })
}

impl PublicationControl {
    /// Correlation for nonowning client/view metadata, never custody authority.
    pub fn output_identity(&self) -> Option<u64> {
        Some(self.candidate_root.root_id())
    }
    /// Immutable VST3 transport quantum in project frames. Same-rate output
    /// requests this many hardware frames; a converting backend verifies its
    /// maximum client slice against this quantum before starting. The callback
    /// still validates its actual extent, including after a route change.
    pub fn plugin_callback_quantum_on_control(&self) -> Option<u16> {
        self.plugin_callback_quantum
    }
    /// Bind only while both exact constructor endpoints remain unexposed.
    /// The backend or headless fixture owns the corresponding delivery bound.
    pub(crate) fn configure_plugin_callback_quantum(
        &mut self,
        callback: &mut Callback,
        frames: u16,
    ) -> Result<(), Invalid> {
        if frames == 0
            || usize::from(frames) > hex_plugin_host::transport::MAX_FRAMES
            || self.plugin_callback_quantum.is_some()
            || callback.plugin_callback_quantum.is_some()
            || self.revision != 0
            || self.native_custody
            || self.status.client_exposed.load(Acquire)
            || self.status.closing.load(Acquire)
            || !callback.transport_setup_open
            || !callback.belongs_to_candidate_root(&self.candidate_root)
            || !callback.matches_native_status(&self.status)
        {
            return Err(Invalid(
                "plugin callback quantum requires its vacant constructor",
            ));
        }
        self.plugin_callback_quantum = Some(frames);
        callback.plugin_callback_quantum = Some(frames);
        Ok(())
    }
    /// Capture the current route header under its sequence lock, then acquire
    /// the admitted-command high-water mark. Returns `None` on a contended or
    /// invalid route snapshot so callers can retry before preparing a graph.
    pub fn capture_fresh_runtime_fence(&self) -> Option<FreshRuntimeFence> {
        capture_runtime_fence(&self.status)
    }

    /// Pair a running Session observation with the matching command/source
    /// fence for conditional control-side preparation. A command batch can
    /// make the observation lag until a later callback publication. The pair
    /// can become stale immediately and does not authorize installation; the
    /// callback must recheck the fence, exact graph and predicted clock. A
    /// zero generation requires the original graph's canonical source context,
    /// since the same scalar also represents legacy unknown context.
    pub fn capture_running_transport_fence(
        &self,
    ) -> Option<(TransportObservation, FreshRuntimeFence)> {
        let fence = capture_runtime_fence(&self.status)?;
        let observation = self.status.transport_observation()?;
        let known_generation = observation.control_generation != 0
            || self.status.recording_source().is_some_and(|source| {
                source.audio_revision == observation.audio_revision
                    && source.context.project_generation == observation.control_generation
            });
        (observation.playing
            && observation.loop_state.is_some()
            && known_generation
            && observation.audio_revision != 0
            && observation.audio_revision == fence.expected_active_revision
            && observation.consumed_command_ticket == fence.admitted_command_ticket
            && self.status.admitted_ticket.load(Acquire) == fence.admitted_command_ticket
            && self.status.consumed_command.load(Acquire) == fence.admitted_command_ticket
            && self.status.last_output_fault_epoch.load(Acquire) == fence.output_fault_epoch
            && self.status.consumed_stop_generation.load(Acquire) == fence.consumed_stop_generation
            && !self.status.render_fault.load(Acquire)
            && !self.status.device_fault.load(Acquire)
            && self.status.pending_stop_marker.load(Acquire) == 0
            && !self.status.closing.load(Acquire))
        .then_some((observation, fence))
    }

    /// Prepare a conditional future anchor from this output's observed running
    /// Session transport. `None` means the observation is unavailable or does
    /// not match the requested source revision and control generation. The
    /// caller retains the revisions independently. The callback rechecks the
    /// exact runtime graphs, clocks, host fence and cancellable admission.
    pub fn prepare_fenced_tempo_anchor(
        &self,
        expected_audio_revision: u64,
        expected_control_generation: u64,
        previous: Arc<hex_engine::PreparedRevision>,
        candidate: Arc<hex_engine::PreparedRevision>,
        cut_performance_frame: u64,
    ) -> Result<Option<FencedTempoCut>, hex_engine::session_playback::SessionPlaybackError> {
        let Some((observation, fence)) = self.capture_running_transport_fence() else {
            return Ok(None);
        };
        if observation.audio_revision != expected_audio_revision
            || observation.control_generation != expected_control_generation
        {
            return Ok(None);
        }
        let Some(loop_state) = observation.loop_state else {
            return Ok(None);
        };
        let anchor =
            hex_engine::session_playback::PreparedTempoAnchor::prepare_from_running_observation(
                previous,
                candidate,
                observation.arrangement_frame,
                observation.performance_frame,
                loop_state,
                cut_performance_frame,
            )?;
        if !self
            .capture_running_transport_fence()
            .is_some_and(|(latest, current)| {
                current == fence
                    && latest.control_generation == expected_control_generation
                    && latest.performance_frame <= cut_performance_frame
            })
        {
            return Ok(None);
        }
        Ok(Some(FencedTempoCut {
            anchor,
            fence,
            output_identity: self.candidate_root.root_id(),
            control_generation: expected_control_generation,
            performance_frame: cut_performance_frame,
        }))
    }

    /// Reserve the ordinary registered-resource subtree before any publication/client exposure.
    /// Configure this capability even for an initially resource-free project so a later sampler
    /// does not reopen the output. Limits grant no custody or complete graph provenance. Legacy
    /// clip PCM uses exact control-owned backing records shared across publications. Each record
    /// releases after its matching graph is destroyed on control. Runtime leases may migrate into tails and
    /// retain their source obligations across returns of the publication which introduced them.
    /// Graph publication for this entire output is permanently worker-owned. Client transport,
    /// Stop and observation remain available; its legacy raw graph-forwarding bridge refuses.
    /// Consumers must route every canonical graph edit, including the resource-free initial graph,
    /// through the worker sink before enabling this capability, as the native resident adapter does.
    pub fn configure_device_resource_publication_on_control(
        &mut self,
        reservation: hex_engine::AudioStorageLedgerReservation,
    ) -> Result<(), &'static str> {
        self.configure_device_resource_publication(reservation, false)
    }

    /// Reserve both resource books on a fresh native output. The verified
    /// constructor owns the shared source root and domain reservation; the
    /// ordinary book is separate and does not include that root. `combined`
    /// must cover both declared limits, with checked arithmetic. These are
    /// caller limits, not an RSS or allocator bound. The combined ceiling
    /// includes one ordinary demand workspace for every permitted graph.
    /// The exposed client retains its verified publication policy until an
    /// explicit `publish_ordinary_handoff_from_verified_on_control` commits
    /// one-way ordinary ownership on the callback.
    /// If construction fails after configuring the
    /// verified root, the output is closed and must be discarded rather than
    /// exposed.
    pub fn configure_combined_publication_on_control(
        &mut self,
        verified: crate::AudioCandidatePublicationReservation,
        ordinary: hex_engine::AudioStorageLedgerReservation,
        ordinary_extents: usize,
        ordinary_metadata_bytes: usize,
        combined: hex_engine::AudioStorageLedgerReservation,
    ) -> Result<(), &'static str> {
        let fits =
            |a: usize, b: usize, limit: usize| a.checked_add(b).is_some_and(|sum| sum <= limit);
        let graph_count = self
            .publications
            .buffer()
            .capacity()
            .checked_add(RETIRED)
            .and_then(|count| count.checked_add(5))
            .ok_or("combined audio reservation exceeded")?;
        let workspace_bytes = ordinary_metadata_bytes
            .checked_mul(graph_count)
            .ok_or("combined audio reservation exceeded")?;
        // One boxed ordinary stream owner plus four extent vectors per graph.
        let workspace_rows = graph_count
            .checked_mul(5)
            .ok_or("combined audio reservation exceeded")?;
        let ordinary_data_rows = ordinary
            .data_rows
            .checked_add(workspace_rows)
            .ok_or("combined audio reservation exceeded")?;
        let ordinary_retained = ordinary
            .max_retained_bytes
            .checked_add(workspace_bytes)
            .ok_or("combined audio reservation exceeded")?;
        let ordinary_peak = ordinary
            .max_construction_peak_bytes
            .checked_add(workspace_bytes)
            .ok_or("combined audio reservation exceeded")?;
        if combined.max_construction_peak_bytes < combined.max_retained_bytes
            || !fits(verified.data_rows, ordinary_data_rows, combined.data_rows)
            || !fits(
                verified.handle_rows,
                ordinary.handle_rows,
                combined.handle_rows,
            )
            || !fits(1, ordinary.root_rows, combined.root_rows)
            || !fits(
                verified.max_retained_data_bytes,
                ordinary_retained,
                combined.max_retained_bytes,
            )
            || !fits(
                verified.max_construction_peak_data_bytes,
                ordinary_peak,
                combined.max_construction_peak_bytes,
            )
        {
            return Err("combined audio reservation exceeded");
        }
        if let Some(bridge) = self.resident_forwarding.as_ref() {
            bridge.begin_combined_configuration(&self.status)?;
        } else {
            return Err("combined reservation requires paired native worker endpoints");
        }
        let result = match self.configure_audio_candidate_publication(verified) {
            Err(error) => Err(error),
            Ok(()) => match self
                .configure_device_resource_publication(ordinary, true)
                .and_then(|()| {
                    self.candidate_root.configure_combined_ordinary_streams(
                        ordinary_extents,
                        ordinary_metadata_bytes,
                    )
                }) {
                Ok(()) => Ok(()),
                Err(error) => {
                    self.request_shutdown();
                    Err(error)
                }
            },
        };
        if let Some(bridge) = self.resident_forwarding.as_ref() {
            bridge.finish_combined_configuration();
        }
        result
    }

    pub(super) fn configure_device_resource_publication(
        &mut self,
        reservation: hex_engine::AudioStorageLedgerReservation,
        shared_verified_root: bool,
    ) -> Result<(), &'static str> {
        if self.device_resources.is_some()
            || self.revision != 0
            || self.status.client_exposed.load(Acquire)
            || self.status.closing.load(Acquire)
            || self.status.publication_pending.load(Acquire)
            || (self.candidate_root.domains_configured() && !shared_verified_root)
            || (shared_verified_root && !self.status.constructor_mode.load(Acquire))
        {
            return Err("device resource publication requires a vacant unexposed ordinary output");
        }
        // One queue, active, deferred, callback-held, control-held and construction owner, plus
        // the existing retirement queue. Each Playback's track/tail/keyboard bound is unchanged.
        let max_graphs = self
            .publications
            .buffer()
            .capacity()
            .checked_add(RETIRED)
            .and_then(|n| n.checked_add(5))
            .ok_or("device resource publication capacity overflow")?;
        let max_obligations = max_graphs
            .checked_mul(hex_engine::native_device_resources::MAX_PLAYBACK_DEVICE_RESOURCE_LEASES)
            .ok_or("device resource publication capacity overflow")?;
        let resources =
            hex_engine::native_device_resources::DeviceResourcePublication::reserve_on_control(
                reservation,
                max_obligations,
                max_graphs,
            )
            .map_err(|_| "device resource publication reservation refused")?;
        let bridge = self
            .resident_forwarding
            .as_ref()
            .map(Arc::clone)
            .unwrap_or_else(resident_forwarding::ResidentForwarding::new);
        if !shared_verified_root {
            bridge.claim_worker_publication_on_control(&self.status)?;
        }
        self.resident_forwarding = Some(bridge);
        self.device_resources = Some(resources);
        Ok(())
    }

    /// Synchronous worker-side inventory of the active resource book. Select this actual root
    /// once in a control union; a prospective publication view already includes the same book.
    pub fn device_resource_admission_on_control(
        &self,
    ) -> Option<hex_engine::native_device_resources::DeviceResourceAdmission<'_>> {
        self.device_resources
            .as_ref()
            .map(|book| book.admission_on_control())
    }

    /// Freeze all graph publication and move the actual resource book into the caller's one
    /// retirement hold before dropping its output. Actual runtimes still own their leases.
    /// Retain this book and the separate legacy allowance until a fresh output acquires the
    /// existing exclusive native lease; endpoint destruction alone does not prove that frontier.
    pub fn take_device_resource_publication_on_shutdown(
        &mut self,
    ) -> Option<hex_engine::native_device_resources::DeviceResourcePublication> {
        self.request_shutdown();
        self.reclaim();
        self.device_resources.take()
    }

    /// Admit the actual current/prospective resource union, then enqueue as the final fallible
    /// prepared-transition step. The caller must reserve canonical commit storage beforehand and
    /// must not report cancellation/rollback after success. Submitted is not callback-applied.
    #[allow(clippy::result_large_err)] // Refusals return the actual graph inline without allocating on failure.
    pub fn publish_with_admission_on_control<E>(
        &mut self,
        playback: Playback,
        policy: PublicationPolicy,
        control_context_generation: u64,
        admit: impl FnOnce(
            hex_engine::native_device_resources::DeviceResourceAdmission<'_>,
        ) -> Result<(), E>,
    ) -> Result<u64, AdmittedPublishError<E>> {
        self.publish_admitted_with_source(
            playback,
            policy,
            control_context_generation,
            None,
            None,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            None,
            admit,
        )
    }
    /// Tag the actual admitted ordinary graph with its captured canonical source.
    /// Existing generation-only methods remain unknown to MIDI recording.
    #[allow(clippy::result_large_err)] // Refusals return the actual graph inline without allocating on failure.
    pub fn publish_with_admission_and_source_on_control<E>(
        &mut self,
        playback: Playback,
        policy: PublicationPolicy,
        source_context: hex_engine::AudioSourceContext,
        admit: impl FnOnce(
            hex_engine::native_device_resources::DeviceResourceAdmission<'_>,
        ) -> Result<(), E>,
    ) -> Result<u64, AdmittedPublishError<E>> {
        self.publish_admitted_with_source(
            playback,
            policy,
            source_context.project_generation,
            Some(source_context),
            None,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            None,
            admit,
        )
    }

    /// Admit and submit a one-way continuous handoff from verified constructor
    /// ownership to ordinary worker-owned resource publication on a
    /// combined-configured output. Preserves the active verified graph and
    /// output while preparing and on refusal; commits ordinary ownership only
    /// when the callback installs this exact graph. After callback install,
    /// verified staging refuses and subsequent ordinary worker publications
    /// reuse this exact output, source root, and resource book.
    #[allow(clippy::result_large_err)] // Refusals return the actual graph inline without allocating on failure.
    pub fn publish_ordinary_handoff_from_verified_on_control<E>(
        &mut self,
        playback: Playback,
        expected_output_identity: u64,
        expected_predecessor: Option<AppliedAudioSource>,
        source_context: hex_engine::AudioSourceContext,
        admit: impl FnOnce(
            hex_engine::native_device_resources::DeviceResourceAdmission<'_>,
        ) -> Result<(), E>,
    ) -> Result<u64, AdmittedPublishError<E>> {
        let refusal = if !self.status.constructor_mode.load(Acquire)
            || !self.candidate_root.ordinary_streams_configured()
            || self.device_resources.is_none()
        {
            Some("ordinary handoff requires a combined-configured output")
        } else if self.status.ordinary_owner_active.load(Acquire) {
            Some("ordinary owner handoff already completed")
        } else if expected_output_identity != self.candidate_root.root_id() {
            Some("ordinary handoff output identity mismatch")
        } else {
            match expected_predecessor {
                Some(predecessor)
                    if predecessor.output_identity != expected_output_identity
                        || self.status.applied_audio_source() != Some(predecessor) =>
                {
                    Some("ordinary handoff predecessor source changed")
                }
                None if self.status.applied_audio_source().is_some()
                    || self.status.applied_revision.load(Acquire) != 0 =>
                {
                    Some("ordinary handoff requires its exact active verified source")
                }
                _ => None,
            }
        };
        if let Some(reason) = refusal {
            return Err(AdmittedPublishError::Publication(PublishError {
                kind: PublishErrorKind::Other,
                reason,
                playback,
            }));
        }
        self.publish_admitted_with_source(
            playback,
            PublicationPolicy::HandoffFromVerified {
                predecessor: expected_predecessor,
            },
            source_context.project_generation,
            Some(source_context),
            None,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            None,
            admit,
        )
    }
    /// Bind a device insert's exact receipt after preparation and before the
    /// graph becomes visible to the callback. The attempt deadline also bounds
    /// the callback's apply decision.
    #[allow(clippy::result_large_err)] // Refusals return the actual graph inline without allocating on failure.
    pub fn publish_device_insert_with_receipt_on_control<E>(
        &mut self,
        playback: Playback,
        policy: PublicationPolicy,
        source_context: hex_engine::AudioSourceContext,
        preparation_deadline: Instant,
        mut bind_receipt: impl FnMut(&CancellablePublicationReceipt),
        admit: impl FnOnce(
            hex_engine::native_device_resources::DeviceResourceAdmission<'_>,
        ) -> Result<(), E>,
    ) -> Result<u64, AdmittedPublishError<E>> {
        if !matches!(policy, PublicationPolicy::DeviceInsert { .. }) {
            return Err(AdmittedPublishError::Publication(PublishError {
                kind: PublishErrorKind::Other,
                reason: "receipt binding requires a device insert",
                playback,
            }));
        }
        self.publish_admitted_with_source(
            playback,
            policy,
            source_context.project_generation,
            Some(source_context),
            Some(FencedAdmission::DeviceInsert(DeviceInsertAdmission {
                bind: &mut bind_receipt,
                approved: None,
                preparation_deadline,
            })),
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            None,
            admit,
        )
    }

    #[allow(clippy::too_many_arguments, clippy::result_large_err)] // One accounted publication with optional native/receipt inputs; refusals return the graph inline.
    pub(super) fn publish_admitted_with_source<E>(
        &mut self,
        playback: Playback,
        policy: PublicationPolicy,
        control_context_generation: u64,
        source_context: Option<hex_engine::AudioSourceContext>,
        admission: Option<FencedAdmission<'_>>,
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))] native_continuation: Option<
            hex_plugin_host::transport::ParameterInputSource,
        >,
        admit: impl FnOnce(
            hex_engine::native_device_resources::DeviceResourceAdmission<'_>,
        ) -> Result<(), E>,
    ) -> Result<u64, AdmittedPublishError<E>> {
        let Some(mut resources) = self.device_resources.take() else {
            return Err(AdmittedPublishError::Publication(PublishError {
                kind: PublishErrorKind::Other,
                reason: "ordinary worker publication is not configured",
                playback,
            }));
        };
        let legacy_pcm = playback
            .resident_pcm_capacity_frames()
            .and_then(|frames| self.legacy_pcm_frames.checked_add(frames));
        let result = if legacy_pcm.is_some() {
            resources.publish_admitted_on_control(playback, |playback, view| {
                if let Err(reason) = admit(view) {
                    return Err(AdmittedPublishError::Admission { reason, playback });
                }
                self.publish_unaccounted_with_native_continuation(
                    playback,
                    policy,
                    control_context_generation,
                    true,
                    false,
                    source_context,
                    admission,
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    native_continuation,
                )
                .map_err(AdmittedPublishError::Publication)
            }).map_err(|error| match error {
                hex_engine::native_device_resources::DeviceResourcePublishError::Resources { playback, .. } =>
                    AdmittedPublishError::Publication(PublishError { kind: PublishErrorKind::Other, reason: "device resource publication storage reservation exceeded", playback }),
                hex_engine::native_device_resources::DeviceResourcePublishError::Publication(error) => error,
            })
        } else {
            Err(AdmittedPublishError::Publication(PublishError {
                kind: PublishErrorKind::Other,
                reason: "resident pcm obligation unavailable or overflowing",
                playback,
            }))
        };
        self.device_resources = Some(resources);
        result
    }

    /// Actual accepted ordinary graph owners still requiring control destruction. Resource-free
    /// graphs count too; a migrated original-source lease can outlive its introducing graph.
    pub fn unreturned_resident_graphs_on_control(&self) -> Option<usize> {
        self.device_resources
            .as_ref()
            .map(|book| book.unreturned_graphs_on_control())
    }
    pub fn outstanding_device_resource_leases(&self) -> usize {
        self.device_resources
            .as_ref()
            .map_or(0, |resources| resources.outstanding_leases())
    }

    #[allow(clippy::result_large_err)] // Refusals return the actual graph inline without allocating on failure.
    pub fn publish(&mut self, playback: Playback) -> Result<u64, PublishError> {
        self.publish_with_policy(playback, PublicationPolicy::InheritRuntime)
    }

    #[allow(clippy::result_large_err)] // Refusals return the actual graph inline without allocating on failure.
    pub fn publish_with_policy(
        &mut self,
        playback: Playback,
        policy: PublicationPolicy,
    ) -> Result<u64, PublishError> {
        self.publish_with_pcm_obligation(playback, policy, 0, false, false)
    }

    /// Publish a prepared graph tagged with the control-side project context
    /// that produced it. Legacy `publish`/`publish_with_policy` retain unknown context,
    /// even though their scalar generation remains zero for existing telemetry.
    #[allow(clippy::result_large_err)] // Refusals return the actual graph inline without allocating on failure.
    pub fn publish_with_policy_and_context_generation(
        &mut self,
        playback: Playback,
        policy: PublicationPolicy,
        control_context_generation: u64,
    ) -> Result<u64, PublishError> {
        self.publish_with_pcm_obligation(playback, policy, control_context_generation, true, false)
    }
    #[allow(clippy::result_large_err)] // Refusals return the actual graph inline without allocating on failure.
    pub(super) fn publish_with_pcm_obligation(
        &mut self,
        playback: Playback,
        policy: PublicationPolicy,
        control_context_generation: u64,
        explicit_control_context: bool,
        forwarded_pcm: bool,
    ) -> Result<u64, PublishError> {
        let Some(mut resources) = self.device_resources.take() else {
            if playback.contains_device_resource_leases()
                || playback.has_foreign_continuation_states()
            {
                return Err(PublishError {
                    kind: PublishErrorKind::Other,
                    reason: "device resource publication was not reserved for this output",
                    playback,
                });
            }
            return self.publish_unaccounted_pcm_obligation(
                playback,
                policy,
                control_context_generation,
                explicit_control_context,
                forwarded_pcm,
                None,
            );
        };
        if forwarded_pcm {
            self.device_resources = Some(resources);
            return Err(PublishError {
                kind: PublishErrorKind::Other,
                reason: "graph publication belongs to the audio worker for this output",
                playback,
            });
        }
        let legacy_pcm = playback
            .resident_pcm_capacity_frames()
            .and_then(|frames| self.legacy_pcm_frames.checked_add(frames));
        let result = if legacy_pcm.is_some() {
            resources.publish_on_control(playback, |playback| {
                self.publish_unaccounted_pcm_obligation(playback, policy, control_context_generation, explicit_control_context, forwarded_pcm, None)
            }).map_err(|error| match error {
                hex_engine::native_device_resources::DeviceResourcePublishError::Resources { playback, .. } => PublishError { kind: PublishErrorKind::Other,
                    reason: "device resource publication storage reservation exceeded", playback,
                },
                hex_engine::native_device_resources::DeviceResourcePublishError::Publication(error) => error,
            })
        } else {
            Err(PublishError {
                kind: PublishErrorKind::Other,
                reason: "resident pcm obligation unavailable or overflowing",
                playback,
            })
        };
        self.device_resources = Some(resources);
        result
    }
    #[allow(clippy::result_large_err)] // Refusals return the actual graph inline without allocating on failure.
    pub(super) fn publish_unaccounted_pcm_obligation(
        &mut self,
        playback: Playback,
        policy: PublicationPolicy,
        control_context_generation: u64,
        explicit_control_context: bool,
        forwarded_pcm: bool,
        source_context: Option<hex_engine::AudioSourceContext>,
    ) -> Result<u64, PublishError> {
        self.publish_unaccounted_with_native_continuation(
            playback,
            policy,
            control_context_generation,
            explicit_control_context,
            forwarded_pcm,
            source_context,
            None,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            None,
        )
    }

    #[allow(clippy::too_many_arguments, clippy::result_large_err)] // Existing ownership plus exact move-only source observation; refusals return the graph inline.
    pub(super) fn publish_unaccounted_with_native_continuation(
        &mut self,
        mut playback: Playback,
        policy: PublicationPolicy,
        control_context_generation: u64,
        explicit_control_context: bool,
        forwarded_pcm: bool,
        source_context: Option<hex_engine::AudioSourceContext>,
        admission: Option<FencedAdmission<'_>>,
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))] native_continuation: Option<
            hex_plugin_host::transport::ParameterInputSource,
        >,
    ) -> Result<u64, PublishError> {
        let (insert, tempo_cut) = match admission {
            Some(FencedAdmission::DeviceInsert(insert)) => (Some(insert), None),
            Some(FencedAdmission::TempoCut(cut)) => (None, Some(cut)),
            None => (None, None),
        };
        if let PublicationPolicy::InheritAtTempoCut(fence) = policy
            && tempo_cut.as_ref().is_none_or(|cut| {
                cut.fence != fence
                    || cut.output_identity != self.candidate_root.root_id()
                    || cut.control_generation != control_context_generation
                    || playback.contains_streamed_sources()
                        && !self.candidate_root.ordinary_streams_configured()
            })
        {
            return Err(PublishError {
                kind: PublishErrorKind::Other,
                reason: "tempo cut requires its original output and ordinary source",
                playback,
            });
        }
        let approved = insert.as_ref().and_then(|insert| insert.approved);
        let preparation_deadline = insert.as_ref().map(|insert| insert.preparation_deadline);
        let preparation_expired =
            || preparation_deadline.is_some_and(|deadline| Instant::now() >= deadline);
        let timed_out = "resident graph edit preparation timed out";
        if preparation_expired() {
            return Err(PublishError {
                kind: PublishErrorKind::Other,
                reason: timed_out,
                playback,
            });
        }
        if approved
            .as_ref()
            .is_some_and(|consent| !self.device_insert_consent_is_current(consent))
        {
            return Err(PublishError {
                kind: PublishErrorKind::Other,
                reason: "device insert confirmation is no longer current",
                playback,
            });
        }
        let native_epoch = match policy {
            PublicationPolicy::InstallUnderNativeEpoch(token)
            | PublicationPolicy::InheritUnderStoppedNative { fence: token, .. } => Some(token),
            _ => None,
        };
        let native_inheritance = match policy {
            PublicationPolicy::InheritUnderStoppedNative { source, fence } => Some((source, fence)),
            _ => None,
        };
        let reason = if native_inheritance.is_some_and(|(source, fence)| {
            !self
                .status
                .stopped_native_source_fence_matches(source, fence)
                || self.status.applied_audio_source().is_some()
                || source_context.is_none_or(|context| {
                    context.project_generation != source.context.project_generation
                })
        }) {
            Some("native inheritance requires its exact stopped active source")
        } else if native_inheritance.is_none()
            && native_epoch.is_some_and(|token| {
                !native_epoch_accepts_successor(
                    self.status.sampler_edit.native_epoch_context(token),
                    source_context,
                ) || self.status.native_effect_source().is_some()
            })
        {
            Some("native successor requires its matching stopped returned source")
        } else if self.status.sampler_edit.native_epoch_active() && native_epoch.is_none() {
            Some("native epoch excludes unrelated graph publication")
        } else if self.status.sampler_edit.active()
            && !matches!(policy, PublicationPolicy::InstallUnderStoppedArrangement(token) | PublicationPolicy::InheritUnderArrangement(token) | PublicationPolicy::InstallUnderNativeEpoch(token) | PublicationPolicy::InheritUnderStoppedNative { fence: token, .. }
                if self.stopped_edit_fence_state(token) == StoppedEditFenceState::Ready)
        {
            Some("stopped edit excludes unrelated graph publication")
        } else if matches!(policy, PublicationPolicy::InstallUnderStoppedArrangement(token) | PublicationPolicy::InheritUnderArrangement(token) | PublicationPolicy::InstallUnderNativeEpoch(token) | PublicationPolicy::InheritUnderStoppedNative { fence: token, .. }
            if self.stopped_edit_fence_state(token) != StoppedEditFenceState::Ready)
        {
            Some("arrangement stopped fence is no longer ready")
        } else if matches!(policy, PublicationPolicy::HandoffFromVerified { .. })
            && (!self.status.constructor_mode.load(Acquire)
                || self.status.ordinary_owner_active.load(Acquire)
                || !self.candidate_root.ordinary_streams_configured())
        {
            Some("ordinary handoff requires an active combined verified output")
        } else if !matches!(policy, PublicationPolicy::HandoffFromVerified { .. })
            && ((self.status.constructor_mode.load(Acquire)
                && !self.status.ordinary_owner_active.load(Acquire))
                || (self.candidate_root.domains_configured()
                    && !self.candidate_root.ordinary_streams_configured()))
        {
            Some("constructor publication does not adopt legacy playback")
        } else if self.status.publication_pending.recording_active() {
            Some("finish recording before replacing playback")
        } else if self.status.closing.load(Acquire) {
            Some("playback is shutting down")
        } else if playback.sample_rate() != self.rate {
            Some("playback rate differs from device")
        } else if playback.contains_streamed_sources()
            && (!self.candidate_root.ordinary_streams_configured() || source_context.is_none())
        {
            Some("streamed playback requires a native media page endpoint")
        } else if self.revision == u64::MAX {
            Some("playback revision exhausted")
        } else {
            None
        };
        if let Some(reason) = reason {
            return Err(PublishError {
                kind: PublishErrorKind::Other,
                reason,
                playback,
            });
        }
        if let PublicationPolicy::DeviceInsert { predecessor, fence } = policy
            && (predecessor.output_identity != self.candidate_root.root_id()
                || predecessor.audio_revision != fence.expected_active_revision
                || self.status.recording_source() != Some(predecessor)
                || source_context.is_none_or(|context| {
                    context.project_generation != predecessor.context.project_generation
                        || context.editor_revision <= predecessor.context.editor_revision
                })
                || playback.session_capability_status() != SessionCapabilityStatus::Available)
        {
            return Err(PublishError {
                kind: PublishErrorKind::Other,
                reason: "device insert requires the exact installed project source",
                playback,
            });
        }
        let Some(pcm_frames) = playback.resident_pcm_capacity_frames() else {
            return Err(PublishError {
                kind: PublishErrorKind::Other,
                reason: "resident pcm capacity overflow",
                playback,
            });
        };
        let Some(next_pcm) =
            self.legacy_pcm_frames
                .checked_add(if forwarded_pcm { 0 } else { pcm_frames })
        else {
            return Err(PublishError {
                kind: PublishErrorKind::Other,
                reason: "resident pcm obligation overflow",
                playback,
            });
        };
        let reserved = if native_epoch.is_some() {
            self.status
                .publication_pending
                .reserve_native_epoch_publication()
        } else {
            self.status.publication_pending.reserve_publication()
        };
        if !reserved {
            return Err(PublishError {
                kind: PublishErrorKind::Other,
                reason: "playback publication already pending",
                playback,
            });
        }
        let revision = self.revision + 1;
        playback.set_device_metering(true);
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        let native_references = match self
            .candidate_root
            .retain_ordinary_native_sources(&playback)
        {
            Ok(Some(_)) if source_context.is_none() => {
                self.status.publication_pending.store(false, Release);
                return Err(PublishError {
                    kind: PublishErrorKind::Other,
                    reason: "native graph publication requires a canonical source context",
                    playback,
                });
            }
            Ok(references) => references,
            Err(failure) => {
                self.status.publication_pending.store(false, Release);
                return Err(PublishError {
                    kind: if failure.is_unavailable() {
                        PublishErrorKind::NativeSourceUnavailable
                    } else {
                        PublishErrorKind::Other
                    },
                    reason: failure.message(),
                    playback,
                });
            }
        };
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        if native_continuation.is_some() && native_references.is_none() {
            self.status.publication_pending.store(false, Release);
            return Err(PublishError {
                kind: PublishErrorKind::NativeSourceUnavailable,
                reason: "native continuation requires its original graph source",
                playback,
            });
        }
        let streams = if self.candidate_root.ordinary_streams_configured() {
            let Some(context) = source_context else {
                self.status.publication_pending.store(false, Release);
                return Err(PublishError {
                    kind: PublishErrorKind::Other,
                    reason: "ordinary stream context unavailable",
                    playback,
                });
            };
            match self
                .candidate_root
                .prepare_ordinary_streams(&playback, revision, context)
            {
                Ok(streams) => Some(streams),
                Err(reason) => {
                    self.status.publication_pending.store(false, Release);
                    return Err(PublishError {
                        kind: PublishErrorKind::Other,
                        reason,
                        playback,
                    });
                }
            }
        } else {
            None
        };
        let cancellation = policy.fresh_fence().map(|fence| {
            let basis = match policy {
                PublicationPolicy::DeviceInsert { predecessor, fence } => {
                    publication_receipt::ReceiptBasis::DeviceInsert(DeviceInsertBasis::new(
                        predecessor,
                        fence,
                        &self.status,
                        approved,
                        preparation_deadline,
                    ))
                }
                _ => tempo_cut.map_or(
                    publication_receipt::ReceiptBasis::None,
                    publication_receipt::ReceiptBasis::TempoCut,
                ),
            };
            CancellablePublicationReceipt::new(
                self.candidate_root.root_id(),
                source_context,
                revision,
                basis,
                fence.route_state_generation,
            )
        });
        let mut publication = Publication {
            pcm_frames,
            forwarded_pcm,
            revision,
            control_context_generation,
            explicit_control_context,
            source_context,
            policy,
            fresh_runtime_cancellation: cancellation.clone().map(PublicationReceiptOwner),
            scheduled_cut_frame: None,
            streams,
            playback: Box::new(playback),
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            native_continuation,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            native_references,
        };
        if preparation_expired() {
            publication.release_streams_on_control(&self.candidate_root);
            self.status.publication_pending.store(false, Release);
            return Err(PublishError {
                kind: PublishErrorKind::Other,
                reason: timed_out,
                playback: *publication.playback,
            });
        }
        if let (Some(insert), Some(receipt)) = (insert, cancellation.as_ref()) {
            (insert.bind)(receipt);
        }
        if let Err(mut publication) = enqueue_legacy(&mut self.publications, publication) {
            publication.release_streams_on_control(&self.candidate_root);
            self.status.publication_pending.store(false, Release);
            return Err(PublishError {
                kind: PublishErrorKind::Other,
                reason: "playback publication queue full",
                playback: *publication.playback,
            });
        }
        self.legacy_pcm_frames = next_pcm;
        self.revision = revision;
        self.status.submitted_revision.store(revision, Release);
        self.latest_fresh_runtime_cancellation = cancellation;
        self.update_resident_cancellation();
        Ok(revision)
    }

    /// Get the latest cancellable submission's exact receipt. Retain it
    /// before another publication can replace this control-side lookup.
    pub fn fresh_runtime_receipt(&self, revision: u64) -> Option<CancellablePublicationReceipt> {
        self.latest_fresh_runtime_cancellation
            .as_ref()
            .filter(|receipt| receipt.revision() == revision)
            .cloned()
    }

    /// Cancel a conditional fresh-runtime publication until the callback's
    /// final Pending-to-Applying CAS. Once Applying wins, wait for the
    /// exact callback terminal before publishing a successor.
    pub fn cancel_fresh_runtime_publication(
        &self,
        revision: u64,
    ) -> FreshRuntimeCancellationResult {
        self.latest_fresh_runtime_cancellation
            .as_ref()
            .filter(|receipt| receipt.revision() == revision)
            .map_or(
                FreshRuntimeCancellationResult::UnknownRevision,
                CancellablePublicationReceipt::cancel,
            )
    }
    /// Bounded reclamation on the control thread, including final PCM drops.
    pub fn reclaim(&mut self) -> usize {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        self.paused_notes.reclaim();
        self.status.retire_stopped_edit_on_control();
        let mut count = 0;
        for _ in 0..RETIRED {
            let publication = self
                .held_control_return
                .take()
                .or_else(|| self.retired.as_mut().and_then(|r| r.pop().ok()));
            let Some(publication) = publication else {
                break;
            };
            match publication {
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                ReturnedPublication::Native(endpoint) => {
                    if let Err(endpoint) = self.candidate_root.ingest_native_endpoint(endpoint) {
                        self.held_control_return = Some(ReturnedPublication::Native(endpoint));
                        break;
                    }
                }
                ReturnedPublication::Domain(pages) => {
                    if let Err(pages) = self.candidate_root.ingest_pages(pages) {
                        self.held_control_return = Some(ReturnedPublication::Domain(pages));
                        break;
                    }
                }
                ReturnedPublication::Eligible(value) => {
                    if let Err(value) = self.candidate_root.ingest_eligible(value) {
                        self.held_control_return = Some(ReturnedPublication::Eligible(value));
                        break;
                    }
                }
                ReturnedPublication::Legacy(mut publication) => {
                    publication.release_streams_on_control(&self.candidate_root);
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    if let Some((references, binding)) = publication
                        .native_references
                        .as_ref()
                        .zip(publication.playback.retired_foreign_instrument())
                    {
                        references.retire_displaced_instrument_on_control(binding);
                    }
                    let frames = publication.pcm_frames;
                    let forwarded = publication.forwarded_pcm;
                    if let Some(resources) = self.device_resources.as_mut() {
                        if let Err(playback) = resources.retire_on_control(publication.playback) {
                            publication.playback = playback;
                            self.held_control_return =
                                Some(ReturnedPublication::Legacy(publication));
                            break;
                        }
                    } else {
                        drop(publication.playback);
                    } // actual owner destruction precedes obligation release
                    if forwarded {
                        if let Some(bridge) = &self.resident_forwarding {
                            bridge.release_pcm_after_drop(frames);
                        }
                    } else {
                        self.legacy_pcm_frames = self
                            .legacy_pcm_frames
                            .checked_sub(frames)
                            .unwrap_or(u64::MAX);
                    }
                }
                ReturnedPublication::Candidate(returned) => {
                    if let Err(returned) = self.candidate_root.ingest(returned) {
                        self.held_control_return = Some(ReturnedPublication::Candidate(returned));
                        break;
                    }
                }
            }
            count += 1;
        }
        crate::lifecycle::request_candidate_cleanup(&self.candidate_root);
        count
    }
    pub(crate) fn enable_native_custody(&mut self) -> Result<(), String> {
        crate::lifecycle::prepare_candidate_cleanup()?;
        self.native_custody = true;
        Ok(())
    }
    pub(crate) fn native_candidate_root(&self) -> crate::candidate_custody::CandidateRoot {
        self.candidate_root.clone()
    }
    /// Opt in only on a vacant native channel. This reserves retained domain
    /// DATA, not native installation, page readiness or complete scalar costs.
    pub fn configure_audio_candidate_domains(
        &mut self,
        reservation: crate::AudioCandidateDomainReservation,
    ) -> Result<(), &'static str> {
        if self.device_resources.is_some()
            || self.status.client_exposed.load(Acquire)
            || !self.native_custody
            || self.revision != 0
            || self.status.closing.load(Acquire)
            || self.status.publication_pending.load(Acquire)
            || self.held_control_return.is_some()
            || self.latest_fresh_runtime_cancellation.is_some()
        {
            return Err("constructor publication requires a vacant native channel");
        }
        self.candidate_root.configure_domains(reservation)
    }
    /// Retains one actual factory pair in a fixed Root cell. Successful custody
    /// may already be closing; the token does not promise publication/readiness.
    pub fn register_audio_candidate_domain(
        &mut self,
        domain: crate::CandidateDomain,
    ) -> Result<crate::AudioCandidateDomainToken, Box<crate::AudioCandidateDomainRefusal>> {
        self.candidate_root.register_domain(domain)
    }
    pub fn retire_audio_candidate_domain(
        &mut self,
        token: crate::AudioCandidateDomainToken,
    ) -> bool {
        self.candidate_root.retire_domain(token)
    }
    /// Moves an already READY scalar report; never joins or destroys endpoints.
    pub fn recover_audio_candidate_domain(
        &mut self,
        token: crate::AudioCandidateDomainToken,
    ) -> Option<hex_stream::PagePoolReport> {
        self.candidate_root.recover_domain(token)
    }
    /// Nonblocking observation. None includes temporary lifecycle contention;
    /// it does not establish zero occupancy or release any reservation.
    pub fn audio_candidate_domain_usage(&self) -> Option<crate::AudioCandidateDomainUsage> {
        self.candidate_root.domain_usage()
    }
    /// Accept disposal-only custody. Queue acceptance is not installation or a
    /// complete-cost claim; the callback currently refuses this path before use.
    pub fn stage_audio_candidate(
        &mut self,
        owner: hex_engine::AudioCandidateOwner,
        domains: Vec<crate::CandidateDomain>,
    ) -> Result<crate::AudioCandidateTicket, crate::AudioCandidateStageRefusal> {
        if !self.native_custody
            || self.status.closing.load(Acquire)
            || owner.sample_rate() != self.rate
            || owner.phase() != hex_engine::AudioCandidateDisposalPhase::Owned
        {
            return Err(crate::AudioCandidateStageRefusal {
                reason: "candidate native custody unavailable",
                owner,
                domains,
            });
        }
        let publications = &mut self.publications;
        self.candidate_root
            .stage(owner, domains, |queued| {
                let Ok(chunk) = publications.write_chunk_uninit(1) else {
                    return Err(queued);
                };
                chunk.fill_from_iter(std::iter::once(QueuedPublication::Candidate(queued)));
                Ok(())
            })
            .map(|(_, ticket)| ticket)
    }
    pub fn recover_audio_candidate(
        &mut self,
        ticket: &crate::AudioCandidateTicket,
    ) -> Option<crate::AudioCandidateRecovery> {
        // Caller polls ordinary return ingestion with reclaim(); this method
        // only moves a READY original bank/receipt, with no destructor or join.
        self.candidate_root.recover(ticket)
    }
    pub fn audio_candidate_custody(&self) -> crate::AudioCandidateCustodyReport {
        self.candidate_root.report()
    }
    pub fn request_shutdown(&mut self) {
        self.status.closing.store(true, Release);
        self.status.keyboard_activation.revoke_reserved_prefix();
        self.status.sampler_keyboard.revoke_all();
        if let Some(reaper) = self.output_transport_reaper.as_ref() {
            reaper.revoke();
        }
        self.status.pending_stop_marker.store(
            self.status.admitted_ticket.load(Acquire).saturating_add(1),
            Release,
        );
    }
}
impl Control {
    /// Caller retains a rejected command; Stop uses the independent priority path.
    pub fn command(&mut self, command: PlaybackCommand) -> Result<(), PlaybackCommand> {
        self.command_with_ticket(command).map(|_| ())
    }
    /// Enqueue without waiting and return a monotonic acknowledgment ticket.
    /// Failed admission does not advance the ticket. `consumed_command` denotes
    /// consumption, not guaranteed application: priority Stop discards commands admitted
    /// before its ticket marker while later commands in the captured prefix follow FIFO.
    /// Playback starts admitted before an output fault that reach consumption
    /// receive `OutputFailed`, even if a replacement has cleared the fault.
    /// Priority Stop can supersede them; later user commands and explicit
    /// seek/stop retain their ordinary ordering.
    /// While a render fault remains latched, starts receive `OutputFailed`
    /// and the failed graph is not rendered; a refused PlayFrom still updates
    /// the recovery cursor unless a later priority Stop supersedes it. Seek
    /// and Stop keep their ordinary semantics. A replacement must clear the
    /// fault before a new start can run the graph.
    pub fn command_with_ticket(
        &mut self,
        command: PlaybackCommand,
    ) -> Result<u64, PlaybackCommand> {
        self.action_with_ticket(PlaybackAction::Transport(command))
            .map_err(|action| match action {
                PlaybackAction::Transport(command) => command,
                PlaybackAction::Session(_)
                | PlaybackAction::LaunchAndPlay(_)
                | PlaybackAction::SetMetronome(_) => command,
            })
    }

    /// Enqueue a fixed-size Session route action without waiting. Actions are
    /// rejected at the callback boundary with a separate ticket/reason status.
    pub fn session_command_with_ticket(
        &mut self,
        command: hex_engine::SessionPlaybackCommand,
    ) -> Result<u64, hex_engine::SessionPlaybackCommand> {
        self.action_with_ticket(PlaybackAction::Session(command))
            .map_err(|action| match action {
                PlaybackAction::Session(command) => command,
                PlaybackAction::Transport(_)
                | PlaybackAction::LaunchAndPlay(_)
                | PlaybackAction::SetMetronome(_) => command,
            })
    }

    /// Atomically enqueue a valid Session clip/scene launch followed by Play.
    pub fn launch_and_play_with_ticket(
        &mut self,
        command: hex_engine::SessionPlaybackCommand,
    ) -> Result<u64, hex_engine::SessionPlaybackCommand> {
        self.action_with_ticket(PlaybackAction::LaunchAndPlay(command))
            .map_err(|action| match action {
                PlaybackAction::LaunchAndPlay(command) | PlaybackAction::Session(command) => {
                    command
                }
                PlaybackAction::Transport(_) | PlaybackAction::SetMetronome(_) => command,
            })
    }

    pub fn action_with_ticket(&mut self, action: PlaybackAction) -> Result<u64, PlaybackAction> {
        endpoints::legacy_enqueue(self, action)
    }
    /// Control-only input allocation; retains only the scalar output activation clock.
    pub fn new_computer_keyboard_input_on_control(
        &self,
    ) -> Result<hex_engine::computer_keyboard::ComputerKeyboardInput, hex_engine::Invalid> {
        hex_engine::computer_keyboard::ComputerKeyboardInput::with_output_activation(Arc::clone(
            &self.status.keyboard_activation,
        ))
    }
    pub fn stop(&self) {
        self.midi_recording.finish();
        self.status.keyboard_activation.revoke_reserved_prefix();
        self.status.sampler_keyboard.revoke_all();
        // Ticket MAX is refused for ordinary commands, so the supported marker
        // encoding (ticket + 1) is always nonzero and cannot wrap.
        self.status
            .pending_stop_marker
            .store(self.command_ticket.saturating_add(1), Release);
        self.processor_audition.invalidate_current_immediately();
    }
    /// Freeze publication, return active/queued graphs over subsequent callbacks.
    /// Control must continue reclaiming until shutdown_complete before stream teardown.
    pub fn request_shutdown(&mut self) {
        self.midi_recording.cancel();
        self.status.keyboard_activation.revoke_reserved_prefix();
        self.status.sampler_keyboard.revoke_all();
        self.status.closing.store(true, Release);
        self.sample_preview.shutdown();
        self.input_monitor.stop();
        self.stop();
    }
    /// Resident compatibility reclamation; Verified clients never own preview PCM.
    pub fn reclaim(&mut self) -> usize {
        self.sample_preview.reclaim();
        self.publication.reclaim()
    }

    /// Drain callback-delivered applied Session route transitions into `out`.
    pub fn drain_session_route_transitions(
        &mut self,
        out: &mut [SessionRouteTransitionObservation],
    ) -> SessionRouteTransitionDrain {
        self.route_transitions.drain(&self.publication.status, out)
    }

    /// True if the route transition observation buffer overflowed since the
    /// previous drain.
    pub fn session_route_transitions_overflowed(&self) -> bool {
        self.publication
            .status
            .route_transition_overflowed
            .load(Acquire)
    }

    /// Total count of route transition observations dropped on this output due
    /// to bounded capacity.
    pub fn session_route_transitions_dropped(&self) -> u64 {
        self.publication
            .status
            .route_transition_dropped
            .load(Acquire)
    }
}

impl Drop for PublicationControl {
    fn drop(&mut self) {
        // The exact worker guard retains its endpoints independently of
        // callback terminal order. Join/final disposal stay on control.
        if let Some(reaper) = self.output_transport_reaper.as_mut() {
            self.status.closing.store(true, Release);
            let _ = reaper.join_on_control();
        }
        if self.status.split_endpoints.load(Acquire) {
            self.request_shutdown();
        }
        if !self.native_custody {
            return;
        }
        self.candidate_root
            .abandon_control(self.retired.take(), self.held_control_return.take());
        #[cfg(all(test, feature = "allocation-probe"))]
        drop(self.endpoint_probe.take());
    }
}

impl PublicationControl {
    /// Original whole-output exchange, with its actual joining worker retained
    /// by this existing control owner. Native device/graph admission remains
    /// unavailable until device insertion and delayed delivery clocks exist.
    /// `process` runs only on the worker and must bound its own IPC/supervision;
    /// it must never load native plugin ABI into Hex.
    pub fn prepare_output_transport_worker_on_control(
        &mut self,
        callback: &mut Callback,
        binding: hex_lattice_contract::ForeignProcessorBinding,
        quantum: u16,
        process: impl FnMut(
            hex_plugin_host::transport::Block,
        ) -> Result<
            hex_plugin_host::transport::Block,
            hex_plugin_host::transport::Fault,
        > + Send
        + 'static,
    ) -> Result<(), Invalid> {
        if self.native_custody
            || self.output_transport_reaper.is_some()
            || binding.root != self.candidate_root.root_id()
            || !callback.belongs_to_candidate_root(&self.candidate_root)
            || !callback.matches_native_status(&self.status)
        {
            return Err(Invalid(
                "output transport owner mismatch or already exposed",
            ));
        }
        let (control, worker) = callback.prepare_output_transport_on_control(
            binding.instance,
            binding.graph,
            binding.epoch,
            quantum,
        )?;
        match crate::plugin_worker::spawn(control, worker, process) {
            Ok(reaper) => {
                self.output_transport_reaper = Some(reaper);
                Ok(())
            }
            Err(_) => {
                // Spawn failed before exposure/dispatch; all endpoint disposal
                // is still on this constructing control lane.
                callback.output_transport = None;
                Err(Invalid("output transport worker could not start"))
            }
        }
    }
    /// Diagnostic/control-fixture observation, never readiness or custody proof.
    pub fn output_transport_completed_blocks_on_control(&self) -> Option<u64> {
        self.output_transport_reaper
            .as_ref()
            .map(crate::plugin_worker::Reaper::completed_blocks)
    }
}

/// Known channel payload/capacity quote for the pinned worker factory.
pub(crate) fn worker_boundary_storage_quote() -> Option<usize> {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    let editor_bytes = crate::foreign_parameter_delivery::editor::constructor_storage_quote()?;
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    let editor_bytes = 0;
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    let paused_note_bytes = input_monitor::constructor_storage_quote()?;
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    let paused_note_bytes = 0;
    std::mem::size_of::<Status>()
        .checked_add(action_outcomes::storage_bytes())?
        .checked_add(SESSION_ROUTE_CAPACITY.checked_mul(std::mem::size_of::<AtomicRouteSlot>())?)?
        .checked_add(crate::sample_preview::constructor_storage_quote()?)?
        .checked_add(crate::sample_recording::storage_quote()?)?
        .checked_add(crate::rolling_recorder::storage_quote()?)?
        .checked_add(input_monitor::constructor_storage_quote()?)?
        .checked_add(paused_note_bytes)?
        .checked_add(crate::gain_recording::constructor_storage_quote()?)?
        .checked_add(crate::native_parameter_input::constructor_storage_quote()?)?
        .checked_add(crate::foreign_parameter_delivery::constructor_storage_quote()?)?
        .checked_add(editor_bytes)?
        .checked_add(crate::midi_recording::constructor_storage_quote()?)?
        .checked_add(route_transitions::constructor_storage_quote()?)?
        .checked_add(spectrum_target::constructor_storage_quote()?)?
        .checked_add(processor_audition::constructor_storage_quote()?)?
        .checked_add(endpoints::constructor_endpoint_storage_quote()?)?
        .checked_add(SpectrumStages::storage_quote()?)
}
pub(crate) fn worker_publication_queue_storage_quote() -> Option<usize> {
    use crate::native_callback_owner::ChannelStorage;
    ChannelStorage::ring_bytes::<QueuedPublication>(1)?
        .checked_add(ChannelStorage::ring_bytes::<ReturnedPublication>(RETIRED)?)?
        .checked_add(ChannelStorage::ring_bytes::<QueuedCommand>(COMMANDS)?)?
        .checked_add(ChannelStorage::ring_bytes::<AudioDiagnosticEvent>(
            audio_diagnostics::TRACE_CAPACITY,
        )?)?
        .checked_add(crate::native_callback_owner::worker_boundary_storage_quote()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_storage_quotes_and_identity_are_bounded() {
        assert!(worker_boundary_storage_quote().is_some());
        assert!(worker_publication_queue_storage_quote().is_some());
        let (control, _callback) = channel(48_000).unwrap();
        assert!(control.output_identity().is_some());
    }
}
