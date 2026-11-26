//! Atomic status mirror published by the prepared callback for control readers.
use super::*;

pub struct Status {
    pub(super) interface_sample_rate: Option<u32>,
    pub(super) buffer_size_frames: Option<u32>,
    pub(super) diagnostics_flags: AtomicU32,
    pub(super) diagnostics_dropped: AtomicU64,
    pub(super) sampler_edit: sampler_edit::Fence,
    pub(super) sampler_keyboard: sampler_keyboard::Mailbox,
    pub(super) keyboard_activation: Arc<hex_engine::computer_keyboard::KeyboardActivationClock>,
    pub(super) project_spectrum: crate::spectrum_worker::ProjectSelectionMailbox,
    pub(super) pending_stop_marker: AtomicU64,
    // Only the callback writes this: even when idle, odd while handling Stop.
    pub(super) consumed_stop_generation: AtomicU64,
    pub(super) admitted_ticket: AtomicU64,
    pub(super) closing: AtomicBool,
    pub(super) publication_pending: admission::PublicationAdmission,
    pub(super) preview_used: AtomicBool,
    pub(super) constructor_mode: AtomicBool,
    pub(super) ordinary_owner_active: AtomicBool,
    pub(super) verified_streaming_mode: AtomicBool,
    pub(super) applied_source: applied_source::AppliedSourceStatus,
    pub(super) recording_source: applied_source::AppliedSourceStatus,
    pub(super) ordinary_native_revision: AtomicU64,
    pub midi_recording_fault: AtomicBool,
    pub(super) route_transition_overflowed: AtomicBool,
    pub(super) route_transition_dropped: AtomicU64,
    pub(super) split_endpoints: AtomicBool,
    pub(super) client_exposed: AtomicBool,
    pub(super) submitted_revision: AtomicU64,
    /// Latched for this output after a device error; recovery opens a new output.
    pub device_fault: AtomicBool,
    pub render_fault: AtomicBool,
    pub(super) render_fault_record: crate::render_fault::RenderFaultRecord,
    pub cpu_meter: crate::cpu_meter::CpuMeter,
    pub callback_max_ns: AtomicU64,
    pub deadline_misses: AtomicU64,
    pub peak_left: AtomicU32,
    pub peak_right: AtomicU32,
    /// Callback-latched maximum peak bits accumulated between UI polls (drained on read).
    pub latched_peak_left: AtomicU32,
    pub latched_peak_right: AtomicU32,
    /// Callback-latched between-poll clip flags: bit 0 = left > 1.0, bit 1 = right > 1.0.
    pub latched_clip_flags: AtomicU32,
    pub applied_revision: AtomicU64,
    /// Highest admitted command consumed, including commands discarded by Stop.
    /// Release publication follows transport state; readers acquire before reading it.
    pub consumed_command: AtomicU64,
    /// Opt-in streamed prefix resolved by ready Play or real Pause/Stop/rejection;
    /// dequeuing a pending Play does not advance this observation.
    pub stream_applied_command_prefix: AtomicU64,
    /// Best-effort render telemetry, never a readiness/ownership receipt.
    pub stream_buffering: AtomicBool,
    /// Monotonic count of callbacks that held streamed playback silent while
    /// the transport was already playing with no Play/action pre-roll pending:
    /// disk pages arrived too late for audible output. Counted per output;
    /// a replacement output starts from zero. Best-effort telemetry.
    pub stream_starved_callbacks: AtomicU64,
    /// Latest streamed launch accepted into actual PendingRoute scheduling.
    pub stream_scheduled_command: AtomicU64,
    /// Latest scheduled launch whose actual due route/render chunk committed.
    pub stream_activated_command: AtomicU64,
    /// Latest schedule observation cancelled or not retained by actual handoff.
    /// This does not claim that an already committed musical route was undone.
    pub stream_invalidated_schedule: AtomicU64,
    pub stream_readiness_refused: AtomicBool,
    /// A demanded active stream reported changed media; retained through Stop
    /// until a replacement graph is installed or the output is replaced;
    /// cached audio alone does not reverify the file.
    pub stream_source_changed: AtomicBool,
    /// Coalescing notification of a demanded audio-page decode/read failure.
    /// The sole control notification observer takes it through
    /// `take_stream_read_failure`; it is not persistent unavailability state.
    pub stream_read_failed: AtomicBool,
    /// Frames committed before the latest BUFFERING gap; zero when complete.
    pub stream_committed_frames: AtomicU64,
    /// Latest rejected command is read through `rejected_action` for a coherent pair.
    pub(super) rejected_action_ticket: AtomicU64,
    pub(super) rejected_action_reason: AtomicU32,
    pub(super) rejected_action_sequence: AtomicU64,
    /// Latest permanently rejected publication; transient handoffs are retried.
    /// Latest rejected graph is read through `rejected_revision` for a coherent pair.
    pub(super) rejected_revision: AtomicU64,
    pub(super) rejected_revision_reason: AtomicU32,
    pub(super) rejected_revision_sequence: AtomicU64,
    /// Scheduled Arrangement cursor: the next content frame the graph renders.
    /// Recording, scheduling and edits use this position.
    pub position: AtomicU64,
    /// Arrangement frame now leaving Main output: `position` delayed by the
    /// graph's Main output latency, mapped back across loop wraps. Use it only
    /// to display the playhead; see `prepared::audible_clock`.
    pub audible_position: AtomicU64,
    // Last callback-published cursor, frozen before fault Stop rewinds the graph.
    pub(super) recovery_position: AtomicU64,
    pub performance_frame: AtomicU64,
    pub transport_epoch: AtomicU64,
    // A capture admitted at or before this epoch crossed an output fault,
    // even if a queued graph subsequently clears the render-fault flag.
    pub(super) last_output_fault_epoch: AtomicU64,
    pub gain_recording_fault: AtomicBool,
    pub native_parameter_input_fault: AtomicBool,
    pub(super) native_parameter_revoked_gesture: AtomicU64,
    pub gain_recording_ready: AtomicBool,
    pub playing: AtomicBool,
    /// Last callback-admitted interactive click state.
    pub metronome_enabled: AtomicBool,
    pub session_capability: AtomicU32,
    pub(super) sampler_console: AtomicBool,
    pub shutdown_complete: AtomicBool,
    pub(super) transport_observation: transport_observation::TransportObservationSlot,
    pub(super) route_sequence: AtomicU64,
    pub(super) route_revision: AtomicU64,
    pub(super) route_count: AtomicU32,
    pub(super) route_capability: AtomicU32,
    pub(super) route_generation: AtomicU64,
    pub(super) route_performance_at_update: AtomicU64,
    pub(super) route_clock_anchor_tag: AtomicU32,
    pub(super) route_clock_cut_frame: AtomicU64,
    pub(super) route_clock_old_beat_raw: AtomicU64,
    pub(super) route_clock_new_beat_raw: AtomicU64,
    pub(super) route_slots: Box<[AtomicRouteSlot]>,
}

impl Default for Status {
    fn default() -> Self {
        let route_slots = (0..MAX_TRACKS)
            .map(|_| AtomicRouteSlot::new())
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Self::with_route_slots(route_slots)
    }
}

impl Status {
    /// Nominal rate of this native output's interface, captured at open.
    /// Headless endpoints do not have an interface.
    pub fn interface_sample_rate(&self) -> Option<u32> {
        self.interface_sample_rate
    }
    /// Configured hardware buffer size in frames when fixed by the output open.
    pub fn buffer_size_frames(&self) -> Option<u32> {
        self.buffer_size_frames
    }

    /// Wait-free callback publication of output sample peaks and between-poll
    /// latched max-peak/clip flags. Nonnegative finite IEEE-754 f32 values
    /// preserve ordering as `u32`, so `fetch_max` is exact and lock-free.
    pub fn record_output_peaks(&self, peaks: [f32; 2]) {
        self.peak_left.store(peaks[0].to_bits(), Relaxed);
        self.peak_right.store(peaks[1].to_bits(), Relaxed);
        let mut clip_mask = 0_u32;
        if peaks[0].is_finite() && peaks[0] >= 0.0 {
            self.latched_peak_left
                .fetch_max(peaks[0].to_bits(), Relaxed);
            if peaks[0] > 1.0 {
                clip_mask |= 1;
            }
        }
        if peaks[1].is_finite() && peaks[1] >= 0.0 {
            self.latched_peak_right
                .fetch_max(peaks[1].to_bits(), Relaxed);
            if peaks[1] > 1.0 {
                clip_mask |= 2;
            }
        }
        if clip_mask != 0 {
            self.latched_clip_flags.fetch_or(clip_mask, Relaxed);
        }
    }

    /// Drain callback-latched between-poll stereo peak maxima and clip flags.
    pub fn take_latched_output_peaks(&self) -> ([f32; 2], [bool; 2]) {
        let left = f32::from_bits(self.latched_peak_left.swap(0, AcqRel));
        let right = f32::from_bits(self.latched_peak_right.swap(0, AcqRel));
        let flags = self.latched_clip_flags.swap(0, AcqRel);
        ([left, right], [flags & 1 != 0, flags & 2 != 0])
    }

    /// Snapshot of the publication reservation, not a guarantee that admission will succeed:
    /// other actors may own admission, and the state can change immediately after this read.
    pub fn publication_in_flight(&self) -> bool {
        self.publication_pending.load(Acquire)
    }

    /// True once a combined-configured output has completed its one-way
    /// verified-to-ordinary owner handoff on the callback.
    pub fn ordinary_owner_active(&self) -> bool {
        self.ordinary_owner_active.load(Acquire)
    }

    #[cfg(feature = "worker-test-support")]
    pub(crate) fn headless_shutdown_requested(&self) -> bool {
        self.closing.load(Acquire)
    }
    #[cfg(test)]
    pub(crate) fn set_split_for_test(&self) {
        self.split_endpoints.store(true, Release);
    }
    #[cfg(test)]
    pub(crate) fn publication_reserve_for_test(&self) -> bool {
        self.publication_pending.reserve_publication()
    }
    #[cfg(test)]
    pub(crate) fn publication_clear_for_test(&self) {
        self.publication_pending.store(false, Release);
    }
    pub(crate) fn preview_permitted(&self) -> bool {
        !self.constructor_mode.load(Acquire)
            && !self.closing.load(Acquire)
            && !self.device_fault.load(Acquire)
            && !self.render_fault.load(Acquire)
    }
    pub(crate) fn mark_preview_used(&self) {
        self.preview_used.store(true, Release);
    }
    pub(crate) fn preview_unused(&self) -> bool {
        !self.preview_used.load(Acquire)
    }
    pub(crate) fn reserve_midi_begin(&self) -> Result<(), &'static str> {
        if self.device_fault.load(Acquire) || self.render_fault.load(Acquire) {
            return Err("midi recording output has failed");
        }
        if self.closing.load(Acquire) {
            return Err("midi recording output is closing");
        }
        if self.verified_streaming_mode.load(Acquire) {
            return Err("midi recording is unavailable for streamed documents");
        }
        if !self.publication_pending.reserve_midi() {
            return Err("playback publication or recording is pending");
        }
        Ok(())
    }
    pub(crate) fn midi_end(&self) {
        self.publication_pending.midi_finished();
    }
    pub(crate) fn reserve_gain_begin(&self) -> Result<bool, &'static str> {
        if self.device_fault.load(Acquire) || self.render_fault.load(Acquire) {
            return Err("gain recording output has failed");
        }
        if self.closing.load(Acquire) {
            return Err("gain recording output is closing");
        }
        if self.constructor_mode.load(Acquire) {
            return Err("gain recording is unavailable for streamed documents");
        }
        if self.publication_pending.midi_active() {
            return Err("finish midi recording before gain recording");
        }
        if self.split_endpoints.load(Acquire) {
            if !self.publication_pending.reserve_gain() {
                return Err("playback publication or gain recording is pending");
            }
            Ok(true)
        } else if self.publication_pending.reserve_legacy_gain() {
            Ok(true)
        } else {
            Err("playback publication or recording is pending")
        }
    }
    pub(crate) fn gain_begin_committed(&self) {
        self.publication_pending.legacy_gain_started();
    }
    pub(crate) fn reserve_native_parameter_input(&self) -> Result<(), &'static str> {
        if self.device_fault.load(Acquire) || self.render_fault.load(Acquire) {
            return Err("native parameter input output has failed");
        }
        if self.closing.load(Acquire) {
            return Err("native parameter input output is closing");
        }
        if !self.publication_pending.reserve_parameter_input() {
            return Err("playback publication or live input is pending");
        }
        Ok(())
    }
    pub(crate) fn native_parameter_input_end(&self) {
        self.publication_pending.parameter_input_finished();
    }
    pub(crate) fn revoke_native_parameter_input(&self, gesture: u64) {
        self.native_parameter_revoked_gesture
            .fetch_max(gesture, AcqRel);
    }
    pub(super) fn native_parameter_input_revoked(&self, gesture: u64) -> bool {
        self.native_parameter_revoked_gesture.load(Acquire) >= gesture
    }
    pub(crate) fn gain_end(&self) {
        self.publication_pending.gain_finished();
    }
    pub(crate) fn candidate_boundary_data_bytes(&self) -> Option<usize> {
        self.route_slots
            .len()
            .checked_mul(std::mem::size_of::<AtomicRouteSlot>())
            .and_then(|bytes| bytes.checked_add(std::mem::size_of_val(self)))
            .and_then(|bytes| {
                bytes.checked_add(std::mem::size_of::<
                    hex_engine::computer_keyboard::KeyboardActivationClock,
                >())
            })
            .and_then(|bytes| bytes.checked_add(2 * std::mem::size_of::<usize>()))
    }
    pub(super) fn with_route_slots(route_slots: Box<[AtomicRouteSlot]>) -> Self {
        Self {
            sampler_edit: Default::default(),
            sampler_keyboard: Default::default(),
            keyboard_activation: Arc::default(),
            project_spectrum: Default::default(),
            pending_stop_marker: AtomicU64::new(0),
            consumed_stop_generation: AtomicU64::new(0),
            admitted_ticket: AtomicU64::new(0),
            closing: AtomicBool::new(false),
            publication_pending: admission::PublicationAdmission::default(),
            preview_used: AtomicBool::new(false),
            constructor_mode: AtomicBool::new(false),
            ordinary_owner_active: AtomicBool::new(false),
            verified_streaming_mode: AtomicBool::new(false),
            applied_source: applied_source::AppliedSourceStatus::default(),
            recording_source: applied_source::AppliedSourceStatus::default(),
            ordinary_native_revision: AtomicU64::new(0),
            midi_recording_fault: AtomicBool::new(false),
            route_transition_overflowed: AtomicBool::new(false),
            route_transition_dropped: AtomicU64::new(0),
            split_endpoints: AtomicBool::new(false),
            client_exposed: AtomicBool::new(false),
            submitted_revision: AtomicU64::new(0),
            device_fault: AtomicBool::new(false),
            render_fault: AtomicBool::new(false),
            render_fault_record: crate::render_fault::RenderFaultRecord::default(),
            interface_sample_rate: None,
            buffer_size_frames: None,
            diagnostics_flags: AtomicU32::new(0),
            diagnostics_dropped: AtomicU64::new(0),
            cpu_meter: crate::cpu_meter::CpuMeter::default(),
            callback_max_ns: AtomicU64::new(0),
            deadline_misses: AtomicU64::new(0),
            peak_left: AtomicU32::new(0),
            peak_right: AtomicU32::new(0),
            latched_peak_left: AtomicU32::new(0),
            latched_peak_right: AtomicU32::new(0),
            latched_clip_flags: AtomicU32::new(0),
            applied_revision: AtomicU64::new(0),
            consumed_command: AtomicU64::new(0),
            stream_applied_command_prefix: AtomicU64::new(0),
            stream_buffering: AtomicBool::new(false),
            stream_starved_callbacks: AtomicU64::new(0),
            stream_scheduled_command: AtomicU64::new(0),
            stream_activated_command: AtomicU64::new(0),
            stream_invalidated_schedule: AtomicU64::new(0),
            stream_readiness_refused: AtomicBool::new(false),
            stream_source_changed: AtomicBool::new(false),
            stream_read_failed: AtomicBool::new(false),
            stream_committed_frames: AtomicU64::new(0),
            rejected_action_ticket: AtomicU64::new(0),
            rejected_action_reason: AtomicU32::new(0),
            rejected_action_sequence: AtomicU64::new(0),
            rejected_revision: AtomicU64::new(0),
            rejected_revision_reason: AtomicU32::new(0),
            rejected_revision_sequence: AtomicU64::new(0),
            position: AtomicU64::new(0),
            audible_position: AtomicU64::new(0),
            recovery_position: AtomicU64::new(0),
            performance_frame: AtomicU64::new(0),
            transport_epoch: AtomicU64::new(1),
            last_output_fault_epoch: AtomicU64::new(0),
            gain_recording_fault: AtomicBool::new(false),
            native_parameter_input_fault: AtomicBool::new(false),
            native_parameter_revoked_gesture: AtomicU64::new(0),
            gain_recording_ready: AtomicBool::new(false),
            playing: AtomicBool::new(false),
            metronome_enabled: AtomicBool::new(false),
            session_capability: AtomicU32::new(SessionCapabilityStatus::Legacy as u32),
            sampler_console: AtomicBool::new(false),
            shutdown_complete: AtomicBool::new(false),
            transport_observation: Default::default(),
            route_sequence: AtomicU64::new(0),
            route_revision: AtomicU64::new(0),
            route_count: AtomicU32::new(0),
            route_capability: AtomicU32::new(SessionCapabilityStatus::Legacy as u32),
            route_generation: AtomicU64::new(0),
            route_performance_at_update: AtomicU64::new(0),
            route_clock_anchor_tag: AtomicU32::new(0),
            route_clock_cut_frame: AtomicU64::new(0),
            route_clock_old_beat_raw: AtomicU64::new(0),
            route_clock_new_beat_raw: AtomicU64::new(0),
            route_slots,
        }
    }
}

impl Status {
    /// Last callback-published arrangement cursor, retained across a fault Stop.
    /// A pending explicit priority Stop takes precedence and returns zero even
    /// before the callback consumes it.
    /// This is a recovery observation, not the current transport position or a
    /// claim that the corresponding audio reached the physical device.
    pub fn recovery_position(&self) -> u64 {
        if !self.closing.load(Acquire) && self.pending_stop_marker.load(Acquire) != 0 {
            return 0;
        }
        self.recovery_position.load(Acquire)
    }
    /// Current diagnostic configuration; this observation grants no authority.
    pub fn audio_diagnostics_flags(&self) -> AudioDiagnosticsFlags {
        AudioDiagnosticsFlags::from_bits(self.diagnostics_flags.load(Relaxed))
    }
    /// Number of callback trace records lost to the bounded diagnostic queue.
    pub fn audio_diagnostics_dropped(&self) -> u64 {
        self.diagnostics_dropped.load(Relaxed)
    }
    /// Read bounded diagnostic telemetry on control/UI, never in a callback.
    pub fn render_fault_snapshot(&self) -> Option<crate::RenderFaultSnapshot> {
        if !self.render_fault.load(Acquire) {
            return None;
        }
        let snapshot = self.render_fault_record.read();
        if self.render_fault.load(Acquire) {
            snapshot
        } else {
            None
        }
    }
    pub(super) fn try_new() -> Result<Self, Invalid> {
        let mut slots = Vec::new();
        slots
            .try_reserve_exact(MAX_TRACKS)
            .map_err(|_| Invalid("could not allocate Session route telemetry"))?;
        slots.extend((0..MAX_TRACKS).map(|_| AtomicRouteSlot::new()));
        Ok(Self::with_route_slots(slots.into_boxed_slice()))
    }

    /// Read one coherent bounded route snapshot into caller-owned storage.
    /// Returns `None` after three contended attempts or when `output` is too
    /// short. Ignore all output entries when it returns `None`.
    pub fn session_routes(&self, output: &mut [SessionRouteEntry]) -> Option<SessionRouteSnapshot> {
        for _ in 0..ROUTE_READER_ATTEMPTS {
            let before = self.route_sequence.load(SeqCst);
            if before & 1 != 0 {
                continue;
            }
            let audio_revision = self.route_revision.load(SeqCst);
            let count = usize::try_from(self.route_count.load(SeqCst)).ok()?;
            let capability =
                SessionCapabilityStatus::from_code(self.route_capability.load(SeqCst))?;
            let route_state_generation = self.route_generation.load(SeqCst);
            let performance_at_update = self.route_performance_at_update.load(SeqCst);
            let clock_anchor = match self.route_clock_anchor_tag.load(SeqCst) {
                0 => None,
                1 => Some(hex_engine::SessionClockAnchor::from_raw(
                    self.route_clock_cut_frame.load(SeqCst),
                    self.route_clock_old_beat_raw.load(SeqCst),
                    self.route_clock_new_beat_raw.load(SeqCst),
                )),
                _ => return None,
            };
            if count > output.len() || count > self.route_slots.len() {
                return None;
            }
            for (index, destination) in output.iter_mut().take(count).enumerate() {
                let slot = self.route_slots.get(index)?;
                let track_id = slot.track_id.load(SeqCst);
                let owner_tag = slot.owner_tag.load(SeqCst);
                let owner_clip_id = slot.owner_clip_id.load(SeqCst);
                let active_origin_tag = slot.active_origin_tag.load(SeqCst);
                let active_performance_anchor_raw = slot.active_performance_anchor_raw.load(SeqCst);
                let active_elapsed_at_anchor_raw = slot.active_elapsed_at_anchor_raw.load(SeqCst);
                let audio_launch_tag = slot.audio_launch_tag.load(SeqCst);
                let audio_launch_frame = slot.audio_launch_frame.load(SeqCst);
                let pending_tag = slot.pending_tag.load(SeqCst);
                let pending_target_tag = slot.pending_target_tag.load(SeqCst);
                let pending_clip_id = slot.pending_clip_id.load(SeqCst);
                let pending_frame = slot.pending_frame.load(SeqCst);
                let pending_beat_raw = slot.pending_beat_raw.load(SeqCst);
                let owner = match owner_tag {
                    1 => SessionRouteOwner::Arrangement,
                    2 => SessionRouteOwner::Session {
                        clip_id: owner_clip_id,
                    },
                    3 => SessionRouteOwner::Silence,
                    _ => return None,
                };
                let active_origin = match active_origin_tag {
                    0 => None,
                    1 => Some(hex_engine::SessionMidiOrigin::from_raw(
                        active_performance_anchor_raw,
                        active_elapsed_at_anchor_raw,
                    )),
                    _ => return None,
                };
                let audio_launch_frame = match audio_launch_tag {
                    0 => None,
                    1 => Some(audio_launch_frame),
                    _ => return None,
                };
                let pending = match pending_tag {
                    0 => None,
                    1 => {
                        let target = match pending_target_tag {
                            1 => SessionRouteTarget::Arrangement,
                            2 => SessionRouteTarget::Session {
                                clip_id: pending_clip_id,
                            },
                            3 => SessionRouteTarget::Silence,
                            _ => return None,
                        };
                        Some(SessionPendingRoute {
                            target,
                            performance_frame: pending_frame,
                            beat_raw: pending_beat_raw,
                        })
                    }
                    _ => return None,
                };
                *destination = SessionRouteEntry {
                    track_id,
                    owner,
                    active_origin,
                    audio_launch_frame,
                    pending,
                };
            }
            let after = self.route_sequence.load(SeqCst);
            if before == after {
                return Some(SessionRouteSnapshot {
                    audio_revision,
                    count,
                    capability,
                    route_state_generation,
                    performance_at_update,
                    clock_anchor,
                });
            }
        }
        None
    }

    /// Read one coherent callback-published transport observation, or `None`
    /// when no supported active runtime is published, the output is closing,
    /// or a bounded read conflicts with a concurrent callback. The result is
    /// telemetry only and can become stale immediately.
    pub fn transport_observation(&self) -> Option<TransportObservation> {
        if self.closing.load(Acquire) {
            return None;
        }
        let value = self.transport_observation.read()?;
        (!self.closing.load(Acquire)).then_some(value)
    }

    pub(super) fn begin_route_snapshot(&self) {
        self.route_sequence.fetch_add(1, SeqCst);
    }

    pub(super) fn end_route_snapshot(&self) {
        self.route_sequence.fetch_add(1, SeqCst);
    }

    pub(super) fn publish_route_header(
        &self,
        revision: u64,
        count: usize,
        capability: SessionCapabilityStatus,
        generation: u64,
        performance_at_update: u64,
        clock_anchor: Option<hex_engine::SessionClockAnchor>,
    ) {
        self.route_revision.store(revision, SeqCst);
        self.route_count.store(count as u32, SeqCst);
        self.route_capability.store(capability as u32, SeqCst);
        self.route_generation.store(generation, SeqCst);
        self.route_performance_at_update
            .store(performance_at_update, SeqCst);
        if let Some(anchor) = clock_anchor {
            self.route_clock_cut_frame.store(anchor.cut_frame, SeqCst);
            self.route_clock_old_beat_raw
                .store(anchor.old_beat.raw(), SeqCst);
            self.route_clock_new_beat_raw
                .store(anchor.new_beat.raw(), SeqCst);
            self.route_clock_anchor_tag.store(1, SeqCst);
        } else {
            self.route_clock_cut_frame.store(0, SeqCst);
            self.route_clock_old_beat_raw.store(0, SeqCst);
            self.route_clock_new_beat_raw.store(0, SeqCst);
            self.route_clock_anchor_tag.store(0, SeqCst);
        }
    }

    pub(super) fn publish_empty_routes(&self) {
        self.metronome_enabled.store(false, Relaxed);
        self.begin_route_snapshot();
        for slot in self.route_slots.iter() {
            slot.clear();
        }
        self.publish_route_header(0, 0, SessionCapabilityStatus::Legacy, 0, 0, None);
        self.sampler_console.store(false, Release);
        self.session_capability
            .store(SessionCapabilityStatus::Legacy as u32, Relaxed);
        self.end_route_snapshot();
    }

    pub(super) fn publish_route_slot(&self, index: usize, route: hex_engine::SessionRouteStatus) {
        let Some(slot) = self.route_slots.get(index) else {
            return;
        };
        slot.track_id.store(route.track.value(), SeqCst);
        match route.owner {
            hex_engine::SessionPlaybackOwner::Arrangement => {
                slot.owner_tag.store(1, SeqCst);
                slot.owner_clip_id.store(0, SeqCst);
            }
            hex_engine::SessionPlaybackOwner::Session { clip } => {
                slot.owner_tag.store(2, SeqCst);
                slot.owner_clip_id.store(clip.value(), SeqCst);
            }
            hex_engine::SessionPlaybackOwner::Silence => {
                slot.owner_tag.store(3, SeqCst);
                slot.owner_clip_id.store(0, SeqCst);
            }
        }
        if let Some(origin) = route.active_origin {
            slot.active_performance_anchor_raw
                .store(origin.performance_anchor.raw(), SeqCst);
            slot.active_elapsed_at_anchor_raw
                .store(origin.elapsed_at_anchor.raw(), SeqCst);
            slot.active_origin_tag.store(1, SeqCst);
        } else {
            slot.active_performance_anchor_raw.store(0, SeqCst);
            slot.active_elapsed_at_anchor_raw.store(0, SeqCst);
            slot.active_origin_tag.store(0, SeqCst);
        }
        if let Some(launch) = route.audio_launch_frame {
            slot.audio_launch_frame.store(launch, SeqCst);
            slot.audio_launch_tag.store(1, SeqCst);
        } else {
            slot.audio_launch_frame.store(0, SeqCst);
            slot.audio_launch_tag.store(0, SeqCst);
        }
        if let Some(pending) = route.pending {
            let (tag, clip_id) = match pending.target {
                hex_engine::SessionRouteTarget::Arrangement => (1, 0),
                hex_engine::SessionRouteTarget::Session { clip } => (2, clip.value()),
                hex_engine::SessionRouteTarget::Silence => (3, 0),
            };
            slot.pending_target_tag.store(tag, SeqCst);
            slot.pending_clip_id.store(clip_id, SeqCst);
            slot.pending_frame.store(pending.performance_frame, SeqCst);
            slot.pending_beat_raw.store(pending.beat.raw(), SeqCst);
            slot.pending_tag.store(1, SeqCst);
        } else {
            slot.pending_tag.store(0, SeqCst);
            slot.pending_target_tag.store(0, SeqCst);
            slot.pending_clip_id.store(0, SeqCst);
            slot.pending_frame.store(0, SeqCst);
            slot.pending_beat_raw.store(0, SeqCst);
        }
    }
}

impl Status {
    pub fn rejected_action(&self) -> Option<(u64, PlaybackActionRejection)> {
        for _ in 0..3 {
            let before = self.rejected_action_sequence.load(SeqCst);
            if before & 1 != 0 {
                continue;
            }
            let ticket = self.rejected_action_ticket.load(SeqCst);
            let reason = self.rejected_action_reason.load(SeqCst);
            let after = self.rejected_action_sequence.load(SeqCst);
            if before == after {
                return (ticket != 0)
                    .then(|| PlaybackActionRejection::from_code(reason))
                    .flatten()
                    .map(|reason| (ticket, reason));
            }
        }
        None
    }

    pub fn rejected_revision(&self) -> Option<(u64, PlaybackHandoffRejection)> {
        for _ in 0..3 {
            let before = self.rejected_revision_sequence.load(SeqCst);
            if before & 1 != 0 {
                continue;
            }
            let revision = self.rejected_revision.load(SeqCst);
            let reason = self.rejected_revision_reason.load(SeqCst);
            let after = self.rejected_revision_sequence.load(SeqCst);
            if before == after {
                return (revision != 0)
                    .then(|| PlaybackHandoffRejection::from_code(reason))
                    .flatten()
                    .map(|reason| (revision, reason));
            }
        }
        None
    }

    pub fn sampler_console_available(&self) -> bool {
        self.sampler_console.load(Acquire)
    }
    pub fn session_capability(&self) -> Option<SessionCapabilityStatus> {
        SessionCapabilityStatus::from_code(self.session_capability.load(Acquire))
    }

    pub(super) fn publish_rejected_action(&self, ticket: u64, reason: PlaybackActionRejection) {
        self.rejected_action_sequence.fetch_add(1, SeqCst);
        self.rejected_action_reason.store(reason as u32, SeqCst);
        self.rejected_action_ticket.store(ticket, SeqCst);
        self.rejected_action_sequence.fetch_add(1, SeqCst);
    }

    pub(super) fn publish_rejected_revision(
        &self,
        revision: u64,
        reason: PlaybackHandoffRejection,
    ) {
        self.rejected_revision_sequence.fetch_add(1, SeqCst);
        self.rejected_revision_reason.store(reason as u32, SeqCst);
        self.rejected_revision.store(revision, SeqCst);
        self.rejected_revision_sequence.fetch_add(1, SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_publishes_rejected_revision_atomically() {
        let status = Status::default();
        assert_eq!(status.applied_revision.load(Acquire), 0);
        assert_eq!(status.rejected_revision(), None);
        status.publish_rejected_revision(7, PlaybackHandoffRejection::RuntimeFenceChanged);
        assert_eq!(
            status.rejected_revision(),
            Some((7, PlaybackHandoffRejection::RuntimeFenceChanged))
        );
    }

    #[test]
    fn output_peak_latch_retains_between_poll_peak_and_clip_across_quieter_callback() {
        let status = Status::default();
        status.record_output_peaks([1.75, 0.4]);
        status.record_output_peaks([0.1, 0.9]);
        // Latest raw sample peaks reflect the second callback.
        assert_eq!(f32::from_bits(status.peak_left.load(Relaxed)), 0.1);
        assert_eq!(f32::from_bits(status.peak_right.load(Relaxed)), 0.9);
        // Drained latch preserves the transient left clip & max peaks across both callbacks.
        assert_eq!(
            status.take_latched_output_peaks(),
            ([1.75, 0.9], [true, false])
        );
        // Second drain without intervening callbacks yields silence and cleared clip flags.
        assert_eq!(
            status.take_latched_output_peaks(),
            ([0.0, 0.0], [false, false])
        );
        // Non-finite or negative values do not corrupt the latch.
        status.record_output_peaks([f32::NAN, -2.0]);
        assert_eq!(
            status.take_latched_output_peaks(),
            ([0.0, 0.0], [false, false])
        );
    }
}
