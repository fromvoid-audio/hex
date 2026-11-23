//! Real-time callback execution, rendering, command consumption, and channel constructors.
use super::*;

#[cfg(feature = "worker-test-support")]
impl Callback {
    /// Read-only headless control observation while the callback is quiescent.
    /// Empty is queue state, not graph/source application or native readiness.
    pub(crate) fn publication_queue_empty_on_control(&self) -> bool {
        self.publications.is_empty()
    }
    /// Quiescent headless observation of the ordinary handoff, including a
    /// consumed publication still awaiting installation. This grants no ACK.
    pub(crate) fn ordinary_publication_idle_on_control(&self) -> bool {
        self.publications.is_empty()
            && self.deferred.is_none()
            && self.deferred_eligible.is_none()
            && !self.status.publication_pending.load(Acquire)
    }
}

pub struct Callback {
    pub(super) diagnostics: audio_diagnostics::TraceWriter,
    pub(super) action_writer: Option<action_outcomes::Writer>,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) native_sources: crate::candidate_custody::NativeEndpointBank,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) paused_native_sources: crate::candidate_custody::NativeEndpointBank,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) paused_monitor_host_frames: Option<usize>,
    pub(super) plugin_callback_quantum: Option<u16>,
    pub(super) output_transport: Option<Box<hex_plugin_host::transport::Callback>>,
    pub(super) output_transport_cancelled: bool,
    pub(super) transport_setup_open: bool,
    pub(super) sample_preview: crate::sample_preview::Callback,
    pub(super) sample_recording: crate::sample_recording::Callback,
    pub(super) input_monitor: input_monitor::Callback,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) paused_notes: input_monitor::Callback,
    pub(super) publications: Consumer<QueuedPublication>,
    pub(super) retired: Producer<ReturnedPublication>,
    pub(super) candidate_root: crate::candidate_custody::CandidateRoot,
    pub(super) commands: Consumer<QueuedCommand>,
    pub(super) status: Arc<Status>,
    pub(super) audible_clock: audible_clock::AudibleClock,
    pub(super) recovery_position_frozen: bool,
    pub(super) recovery_seek_pending: Option<u64>,
    pub(super) device_fault_stopped: bool,
    pub(super) device_fault_fenced_in_block: bool,
    pub(super) rate: u32,
    pub(super) spectrum_target: spectrum_target::SpectrumTargetCallback,
    pub(super) processor_audition: processor_audition::ProcessorAuditionTargetCallback,
    pub(super) processor_audition_lease: Option<processor_audition::ProcessorAuditionTarget>,
    pub(super) processor_audition_rejected_epoch: u64,
    pub(super) gain_recording: crate::gain_recording::Callback,
    pub(super) native_parameter_input: Box<crate::native_parameter_input::Callback>,
    pub(super) foreign_parameter_delivery: crate::foreign_parameter_delivery::Callback,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) foreign_editor_delivery: crate::foreign_parameter_delivery::editor::Callback,
    pub(super) midi_recording: crate::midi_recording::Callback,
    pub(super) spectrum_capture: Option<SpectrumCapture>,
    pub(super) spectrum_stages: SpectrumStages,
    pub(super) active: Option<ActivePublication>,
    #[cfg(test)]
    pub(super) test_admit_candidate: bool,
    pub(super) deferred: Option<Publication>,
    pub(super) deferred_eligible: Option<crate::candidate_custody::QueuedEligibleCandidate>,
    pub(super) domain_pages: [Option<crate::candidate_custody::DomainEnvelope>; 2],
    pub(super) stream_pending_play: Option<u64>,
    // Credits remain reserved until publication, so no more than the ledger
    // capacity can accumulate before this callback publishes and flushes them.
    pub(super) stream_outcomes: [Option<(u64, action_outcomes::Outcome)>; ACTION_OUTCOME_CAPACITY],
    pub(super) stream_outcome_count: usize,
    pub(super) transport_fade: Option<transport_fade::PendingFade>,
    pub(super) yield_stream_successor_once: bool,
    pub(super) current_quantum_pins: Option<crate::candidate_custody::QuantumPins>,
    pub(super) device_insert_fade_pins: Option<crate::candidate_custody::QuantumPins>,
    pub(super) cold_device_fade: Option<device_insert_consent::ColdDeviceFade>,
    pub(super) stream_prefix: u64,
    // Defensive retention if a push unexpectedly fails; never drop graph ownership.
    pub(super) held_retirement: Option<ReturnedPublication>,
    pub(super) terminal_disposal_pending: bool,
    pub(super) published_route_key: Option<(u64, u64, Option<hex_engine::SessionClockAnchor>)>,
    #[cfg(test)]
    pub(super) route_slot_visits: usize,
    #[cfg(test)]
    pub(super) test_transient_handoff_refusals: usize,
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(super) test_eligible_transfer_allowance: Option<usize>,
    #[cfg(test)]
    pub(super) test_after_fresh_fence_validation: Option<std::sync::Arc<std::sync::Barrier>>,
    #[cfg(test)]
    pub(super) test_before_fresh_runtime_apply: Option<std::sync::Arc<std::sync::Barrier>>,
    #[cfg(test)]
    pub(super) test_after_fresh_runtime_apply_decision: Option<std::sync::Arc<std::sync::Barrier>>,
    #[cfg(test)]
    pub(super) test_after_spectrum_target_boundary: Option<std::sync::Arc<std::sync::Barrier>>,
    #[cfg(test)]
    pub(super) test_before_action_outcome_status: Option<std::sync::Arc<std::sync::Barrier>>,
}

pub(crate) fn native_endpoint_bank_storage_quote() -> Option<usize> {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        crate::candidate_custody::NativeEndpointBank::data_quote()?.checked_mul(2)
    }
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        Some(0)
    }
}

/// Allocate channels/telemetry on the control side. Initial graph must be
/// published before playback; an empty provider emits silence.
pub fn channel(rate: u32) -> Result<(Control, Callback), Invalid> {
    channel_with_spectrum(rate, None)
}

/// Construct the prepared callback with an optional prebuilt bounded spectrum
/// producer. The worker and queue are created off the audio thread.
pub fn channel_with_spectrum(
    rate: u32,
    capture: Option<SpectrumCapture>,
) -> Result<(Control, Callback), Invalid> {
    channel_with_interface(rate, capture, None)
}

pub(crate) fn channel_with_interface(
    rate: u32,
    capture: Option<SpectrumCapture>,
    interface_sample_rate: Option<u32>,
) -> Result<(Control, Callback), Invalid> {
    if !(8_000..=192_000).contains(&rate) {
        return Err(Invalid("unsupported playback sample rate"));
    }
    let (input_monitor, callback_input_monitor) = input_monitor::channel(rate);
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    let (paused_notes, callback_paused_notes) = input_monitor::channel(rate);
    let (mut sample_preview, callback_preview) = crate::sample_preview::channel(rate);
    let (sample_recording, callback_recording) = crate::sample_recording::channel(rate);
    let (publications, incoming) = RingBuffer::new(1);
    let (returning, retired) = RingBuffer::new(RETIRED);
    let (commands, events) = RingBuffer::new(COMMANDS);
    let (trace_writer, trace_reader) = audio_diagnostics::channel();
    let mut status = Status::try_new()?;
    status.interface_sample_rate = interface_sample_rate;
    let status = Arc::new(status);
    let (spectrum_target, callback_target) = spectrum_target::channel();
    let (processor_audition, callback_audition) = processor_audition::channel();
    let (mut gain_recording, callback_gain_recording) = crate::gain_recording::channel();
    let (mut native_parameter_input, callback_native_parameter_input) =
        crate::native_parameter_input::channel();
    let (foreign_parameter_delivery, callback_foreign_parameter_delivery) =
        crate::foreign_parameter_delivery::channel();
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    let (foreign_editor_delivery, callback_foreign_editor_delivery) =
        crate::foreign_parameter_delivery::editor::channel();
    let (mut midi_recording, callback_midi_recording) = crate::midi_recording::channel();
    sample_preview.bind_status(status.clone());
    gain_recording.bind_status(status.clone());
    native_parameter_input.bind_status(status.clone());
    midi_recording.bind_status(status.clone());
    let candidate_root = crate::candidate_custody::CandidateRoot::new(RETIRED).map_err(Invalid)?;
    let command_storage_bytes = commands
        .buffer()
        .capacity()
        .checked_mul(std::mem::size_of::<QueuedCommand>())
        .and_then(|n| n.checked_add(std::mem::size_of_val(commands.buffer())))
        .and_then(|n| n.checked_add(trace_writer.storage_bytes()?))
        .ok_or(Invalid("command queue storage overflow"))?;
    Ok((
        Control {
            diagnostics: trace_reader,
            action_admission: None,
            sample_preview,
            sample_recording,
            input_monitor,
            commands,
            spectrum_target,
            processor_audition,
            gain_recording,
            native_parameter_input,
            midi_recording,
            command_ticket: 0,
            publication: PublicationControl {
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                paused_notes,
                foreign_parameter_delivery: Some(foreign_parameter_delivery),
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                foreign_editor_delivery: Some(foreign_editor_delivery),
                plugin_callback_quantum: None,
                output_transport_reaper: None,
                candidate_root: candidate_root.clone(),
                native_custody: false,
                held_control_return: None,
                publications,
                retired: Some(retired),
                status: status.clone(),
                rate,
                revision: 0,
                command_storage_bytes,
                legacy_pcm_frames: 0,
                device_resources: None,
                resident_forwarding: None,
                #[cfg(all(test, feature = "allocation-probe"))]
                endpoint_probe: None,
                latest_fresh_runtime_cancellation: None,
            },
        },
        Callback {
            diagnostics: trace_writer,
            action_writer: None,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            native_sources: crate::candidate_custody::NativeEndpointBank::new().map_err(Invalid)?,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            paused_native_sources: crate::candidate_custody::NativeEndpointBank::new()
                .map_err(Invalid)?,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            paused_monitor_host_frames: None,
            plugin_callback_quantum: None,
            output_transport: None,
            output_transport_cancelled: false,
            transport_setup_open: true,
            sample_preview: callback_preview,
            sample_recording: callback_recording,
            input_monitor: callback_input_monitor,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            paused_notes: callback_paused_notes,
            candidate_root,
            publications: incoming,
            retired: returning,
            commands: events,
            status,
            audible_clock: audible_clock::AudibleClock::new(),
            recovery_position_frozen: false,
            recovery_seek_pending: None,
            device_fault_stopped: false,
            device_fault_fenced_in_block: false,
            rate,
            spectrum_target: callback_target,
            processor_audition: callback_audition,
            processor_audition_lease: None,
            processor_audition_rejected_epoch: 0,
            gain_recording: callback_gain_recording,
            native_parameter_input: callback_native_parameter_input,
            foreign_parameter_delivery: callback_foreign_parameter_delivery,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            foreign_editor_delivery: callback_foreign_editor_delivery,
            midi_recording: callback_midi_recording,
            spectrum_capture: capture,
            spectrum_stages: SpectrumStages::new()?,
            active: None,
            #[cfg(test)]
            test_admit_candidate: false,
            deferred: None,
            deferred_eligible: None,
            domain_pages: std::array::from_fn(|_| None),
            stream_pending_play: None,
            stream_outcomes: [None; ACTION_OUTCOME_CAPACITY],
            stream_outcome_count: 0,
            transport_fade: None,
            yield_stream_successor_once: false,
            current_quantum_pins: None,
            device_insert_fade_pins: None,
            cold_device_fade: None,
            stream_prefix: 0,
            held_retirement: None,
            terminal_disposal_pending: false,
            published_route_key: None,
            #[cfg(test)]
            route_slot_visits: 0,
            #[cfg(test)]
            test_transient_handoff_refusals: 0,
            #[cfg(all(test, feature = "allocation-probe"))]
            test_eligible_transfer_allowance: None,
            #[cfg(test)]
            test_after_fresh_fence_validation: None,
            #[cfg(test)]
            test_before_fresh_runtime_apply: None,
            #[cfg(test)]
            test_after_fresh_runtime_apply_decision: None,
            #[cfg(test)]
            test_after_spectrum_target_boundary: None,
            #[cfg(test)]
            test_before_action_outcome_status: None,
        },
    ))
}

impl Callback {
    pub(super) fn reject_paused_monitors(&mut self, reason: input_monitor::MonitorStatus) {
        self.input_monitor.reject_paused(reason);
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        self.paused_notes.reject_paused(reason);
    }
    /// Construct an original whole-output transport profile before callback
    /// exposure. This does not admit a device, graph node or native worker.
    /// Native resource admission remains unavailable until device insertion
    /// and delayed delivery clocks exist. Spectrum/gain capture and native
    /// parameter input are unsupported in this original whole-output profile;
    /// ordinary output is unchanged.
    pub(crate) fn prepare_output_transport_on_control(
        &mut self,
        instance: u64,
        graph_revision: u64,
        epoch: u64,
        quantum: u16,
    ) -> Result<
        (
            hex_plugin_host::transport::Control,
            hex_plugin_host::transport::Worker,
        ),
        Invalid,
    > {
        if self.output_transport.is_some()
            || !self.transport_setup_open
            || self.active.is_some()
            || self.spectrum_capture.is_some()
            || self.status.client_exposed.load(Acquire)
            || self.status.closing.load(Acquire)
            || epoch != self.status.transport_epoch.load(Acquire)
        {
            return Err(Invalid(
                "output transport requires an unexposed empty callback",
            ));
        }
        let binding = hex_lattice_contract::ForeignProcessorBinding {
            root: self.candidate_root.root_id(),
            instance,
            graph: graph_revision,
            epoch,
        };
        let (control, callback, worker) = hex_plugin_host::transport::channel(binding, quantum)
            .map_err(|_| Invalid("invalid output transport configuration"))?;
        self.output_transport = Some(Box::new(callback));
        Ok((control, worker))
    }

    pub(crate) fn output_transport_quantum(&self) -> Option<usize> {
        self.output_transport
            .as_ref()
            .map(|transport| transport.quantum_frames())
    }

    pub(crate) fn revoke_output_transport(&mut self) {
        self.revoke_native_effects();
        self.revoke_forwarded_output_transport();
    }

    pub(super) fn revoke_forwarded_output_transport(&mut self) {
        if let Some(transport) = self.output_transport.as_mut() {
            transport.revoke();
            self.output_transport_cancelled = true;
        }
    }

    pub(crate) fn stage_output_transport(&mut self, frames: &mut [hex_engine::Frame]) -> bool {
        if self.status.closing.load(Acquire) {
            self.revoke_output_transport();
        }
        if self.output_transport_cancelled {
            frames.fill([0.0; 2]);
            if self.status.playing.load(Relaxed) {
                if let Some(transport) = self.output_transport.as_mut() {
                    transport.reject(hex_plugin_host::transport::Fault::Revoked);
                }
                return false;
            }
            return true;
        }
        let Some(transport) = self.output_transport.as_mut() else {
            return true;
        };
        if transport.binding().graph != self.status.applied_revision.load(Acquire)
            || transport.binding().epoch != self.status.transport_epoch.load(Acquire)
        {
            transport.reject(hex_plugin_host::transport::Fault::Identity);
            return false;
        }
        if self.gain_recording.has_take()
            || self.native_parameter_input.has_input()
            || frames.len() > hex_plugin_host::transport::MAX_FRAMES
        {
            transport.reject(hex_plugin_host::transport::Fault::Protocol);
            return false;
        }
        let mut input = [[0.0; 2]; hex_plugin_host::transport::MAX_FRAMES];
        input[..frames.len()].copy_from_slice(frames);
        // This finite stereo-output profile has no MIDI destination. Refuse
        // output events rather than silently claiming they reached a device.
        transport
            .stage(&input[..frames.len()], &[], frames)
            .is_ok_and(|events| events.as_slice().is_empty())
    }

    pub(crate) fn finish_output_transport(&mut self, accepted: bool) -> OutputTransportDelivery {
        use OutputTransportDelivery::{Accepted, Cancelled, Refused};
        let Some(transport) = self.output_transport.as_ref() else {
            return Accepted;
        };
        if transport
            .fault()
            .is_some_and(|fault| fault != hex_plugin_host::transport::Fault::Revoked)
        {
            return Refused;
        }
        // Control may cancel after staging/conversion. Recheck its real owner
        // signals at the last boundary before any request can reach the worker.
        if self.status.closing.load(Acquire) || self.status.pending_stop_marker.load(Acquire) != 0 {
            self.revoke_output_transport();
            return Cancelled;
        }
        if self.output_transport_cancelled {
            return if self.status.playing.load(Relaxed) {
                Refused
            } else {
                Cancelled
            };
        }
        match self.output_transport.as_mut() {
            Some(transport) => {
                if transport.finish(accepted).is_ok() {
                    Accepted
                } else {
                    Refused
                }
            }
            _ => Refused,
        }
    }
    pub(crate) fn output_transport_error(&self) -> Option<Invalid> {
        self.output_transport
            .as_ref()
            .and_then(|transport| transport.fault())
            .map(|fault| Invalid(fault.message()))
            .or_else(|| {
                self.native_effects_revoked()
                    .then_some(Invalid("native effect callback refused"))
            })
    }

    pub(crate) fn native_spectrum_receipt(
        &self,
    ) -> Option<Option<crate::spectrum_worker::NativeSpectrumReceipt>> {
        self.spectrum_capture
            .as_ref()
            .map_or(Some(None), |capture| capture.native_receipt().map(Some))
    }
    pub(crate) fn constructor_channel_storage(
        &self,
    ) -> Option<crate::native_callback_owner::ChannelStorage> {
        // Worker endpoints retire through the existing guards/control owner.
        // Ring custody still does not grant native graph/device admission or
        // supply the delayed delivery clocks required for real device insertion.
        if self.output_transport.is_some() {
            return None;
        }
        let outcome_storage = if self.action_writer.is_some() {
            crate::native_callback_owner::ChannelStorage {
                data_bytes: action_outcomes::storage_bytes(),
                shared_groups: 1,
                shared_handles: 3,
            }
        } else {
            crate::native_callback_owner::ChannelStorage::default()
        };
        let storage = self
            .sample_preview
            .constructor_channel_storage()?
            .plus(crate::sample_recording::Control::callback_storage(
                &self.sample_recording,
            )?)?
            .plus(self.input_monitor.constructor_channel_storage()?)?
            .plus(self.gain_recording.constructor_channel_storage()?)?
            .plus(self.native_parameter_input.constructor_channel_storage()?)?
            .plus(
                self.foreign_parameter_delivery
                    .constructor_channel_storage()?,
            )?
            .plus(self.midi_recording.constructor_channel_storage()?)?
            .plus(self.spectrum_target.constructor_channel_storage()?)?
            .plus(self.processor_audition.constructor_channel_storage()?)?
            .plus(outcome_storage)?
            .plus(crate::native_callback_owner::ChannelStorage {
                data_bytes: self.spectrum_stages.heap_data_bytes()?,
                shared_groups: 0,
                shared_handles: 0,
            })?;
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        let storage = storage
            .plus(self.foreign_editor_delivery.constructor_channel_storage()?)?
            .plus(self.paused_notes.constructor_channel_storage()?)?;
        Some(storage)
    }

    pub(super) fn install_candidate(
        &mut self,
        value: crate::candidate_custody::QueuedCandidate,
    ) -> Result<(), crate::candidate_custody::QueuedCandidate> {
        #[cfg(test)]
        let allowed = self.test_admit_candidate;
        #[cfg(not(test))]
        let allowed = false;
        if !allowed
            || self.active.is_some()
            || self.deferred.is_some()
            || self.status.closing.load(Acquire)
        {
            return Err(value);
        }
        let active =
            crate::candidate_custody::ActiveCandidate::install(value, &self.candidate_root)?;
        self.active = Some(ActivePublication::Candidate(active));
        self.status.publication_pending.store(false, Release);
        Ok(())
    }
    pub(crate) fn native_endpoint_bank_data_bytes(&self) -> Option<usize> {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            self.native_sources
                .data_bytes()?
                .checked_add(self.paused_native_sources.data_bytes()?)
        }
        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        {
            Some(0)
        }
    }
    pub(crate) fn matches_native_status(&self, status: &Arc<Status>) -> bool {
        Arc::ptr_eq(&self.status, status)
    }
    pub(crate) fn has_action_writer(&self) -> bool {
        self.action_writer.is_some()
    }
    pub(crate) fn action_writer_identity(&self) -> Option<usize> {
        self.action_writer
            .as_ref()
            .map(action_outcomes::Writer::identity)
    }
    /// Consume the matched producer and writer only under the exact native
    /// root's post-guard, post-stream-handle release authority.
    pub(crate) fn retire_action_outcomes_on_control(
        &mut self,
        client: &mut ClientEndpoints,
        authority: &crate::native_callback_owner::FinalReleaseAuthority,
    ) -> Result<(), ()> {
        if !authority.matches(self.candidate_root.root_id())
            || !client.matches(&self.status)
            || !client.matches_retired_scalar_output(
                Arc::as_ptr(&self.status) as usize,
                self.candidate_root.root_id(),
            )
        {
            return Err(());
        }
        let Some(writer) = self.action_writer.take() else {
            return Err(());
        };
        let Some(admission) = client.take_action_admission() else {
            self.action_writer = Some(writer);
            return Err(());
        };
        match writer.interrupt_after_quiescence(admission) {
            Ok(()) => Ok(()),
            Err((writer, admission)) => {
                self.action_writer = Some(writer);
                client.restore_action_admission(admission);
                Err(())
            }
        }
    }
    pub(crate) fn candidate_root_for_native_boundary(
        &self,
    ) -> crate::candidate_custody::CandidateRoot {
        self.candidate_root.clone()
    }
    pub(crate) fn belongs_to_candidate_root(
        &self,
        root: &crate::candidate_custody::CandidateRoot,
    ) -> bool {
        self.candidate_root.same_root(root)
    }
    pub(crate) fn has_active_ordinary_streams(&self) -> bool {
        matches!(&self.active, Some(ActivePublication::Legacy(publication)) if publication.streams.is_some())
    }
    /// Both callback carriers and the actual stream handle are terminal-owned.
    /// Release only the ordinary graph's source associations here; its actual
    /// Playback/resource-book owners still follow their existing destructors.
    pub(crate) fn release_final_ordinary_streams_on_control(
        &mut self,
        authority: &crate::native_callback_owner::FinalReleaseAuthority,
    ) -> bool {
        if !authority.matches(self.candidate_root.root_id()) {
            return false;
        }
        if let Some(ActivePublication::Legacy(publication)) = self.active.as_mut() {
            publication.release_streams_on_control(&self.candidate_root);
            // Both carriers and the actual backend are terminal here. The
            // remaining graph is inert metadata; releasing native references
            // cannot admit reuse because this Root is already closing. Keep
            // the original callback Box/resources in their terminal custody.
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            drop(publication.native_references.take());
        }
        true
    }
    pub(crate) fn has_active_candidate(&self) -> bool {
        matches!(
            self.active,
            Some(ActivePublication::Candidate(_) | ActivePublication::Eligible(_))
        )
    }
    pub(crate) fn extract_final_candidate_on_control(
        &mut self,
        authority: &crate::native_callback_owner::FinalReleaseAuthority,
        destination: &mut Option<crate::candidate_custody::FinalActiveReturn>,
    ) -> bool {
        if destination.is_some() || !authority.matches(self.candidate_root.root_id()) {
            return false;
        }
        if !self.has_active_candidate() {
            return true;
        }
        // Prevalidated destination and authority; no provider/destructor occurs
        // between taking the active owner and securing its persistent terminal slot.
        match self.active.take() {
            Some(ActivePublication::Candidate(active)) => match active.return_final(authority) {
                Ok(returned) => *destination = Some(returned),
                Err(active) => {
                    self.active = Some(ActivePublication::Candidate(active));
                    return false;
                }
            },
            Some(ActivePublication::Eligible(active)) => match active.return_final(authority) {
                Ok(returned) => {
                    *destination = Some(crate::candidate_custody::FinalActiveReturn::Eligible(
                        returned,
                    ))
                }
                Err(active) => {
                    self.active = Some(ActivePublication::Eligible(active));
                    return false;
                }
            },
            other => {
                self.active = other;
                return false;
            }
        }
        true
    }
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(super) fn apply_candidate_action(&mut self, action: PlaybackAction) {
        if let Some(ActivePublication::Candidate(active)) = self.active.as_mut()
            && active.owner.test_apply_action(action).is_err()
        {
            self.status.device_fault.store(true, Relaxed);
        }
    }
    pub(crate) fn close_candidate_custody_on_control(&mut self) {
        self.status.closing.store(true, Release);
        self.release_device_insert_fade_pins();
        self.candidate_root.close();
    }
    pub(crate) fn has_domain_pages(&self) -> bool {
        self.domain_pages.iter().any(Option::is_some)
    }
    pub(crate) fn extract_domain_pages_on_control(
        &mut self,
        destination: &mut [Option<crate::candidate_custody::DomainEnvelope>; 2],
    ) -> bool {
        if destination.iter().any(Option::is_some)
            || self.domain_pages.iter().enumerate().any(|(index, pages)| {
                pages.as_ref().is_some_and(|pages| {
                    pages.index_for(self.candidate_root.root_id()) != Some(index)
                })
            })
        {
            return false;
        }
        for (from, to) in self.domain_pages.iter_mut().zip(destination) {
            *to = from.take();
            if let Some(pages) = to.as_ref() {
                pages.departed_callback();
            }
        }
        true
    }
    pub(crate) fn drain_candidate_custody_on_control(&mut self) -> bool {
        self.close_candidate_custody_on_control();
        if self.terminal_disposal_pending {
            return false;
        }
        // Persistent held custody precedes any destructor. A failed legacy
        // destructor latches Pending, just like the outer callback carrier.
        // Candidate carriers only move into the exact root/slot association.
        for _ in 0..2 {
            // one existing held return plus the one-slot input ring
            if self.held_retirement.is_none() {
                self.held_retirement = self
                    .deferred_eligible
                    .take()
                    .map(|value| ReturnedPublication::Eligible(value.reject_before_install()))
                    .or_else(|| match self.publications.pop().ok()? {
                        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                        QueuedPublication::NativeEpoch { .. } => None,
                        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                        QueuedPublication::Native(endpoint) => {
                            Some(ReturnedPublication::Native(endpoint))
                        }
                        QueuedPublication::Domain(pages) => {
                            Some(ReturnedPublication::Domain(pages))
                        }
                        QueuedPublication::Eligible(value) => {
                            Some(ReturnedPublication::Eligible(value.reject_before_install()))
                        }
                        QueuedPublication::Legacy(value) => {
                            Some(ReturnedPublication::Legacy(value))
                        }
                        QueuedPublication::Candidate(value) => Some(
                            ReturnedPublication::Candidate(value.reject_before_install()),
                        ),
                    });
            }
            if matches!(
                self.held_retirement.as_ref(),
                Some(ReturnedPublication::Legacy(_))
            ) {
                if let Some(ReturnedPublication::Legacy(publication)) =
                    self.held_retirement.as_mut()
                {
                    publication.release_streams_on_control(&self.candidate_root);
                }
                self.terminal_disposal_pending = true;
                drop(self.held_retirement.take());
                self.terminal_disposal_pending = false;
            } else if let Some(value) = self.held_retirement.take() {
                let refused = match value {
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    ReturnedPublication::Native(endpoint) => self
                        .candidate_root
                        .ingest_native_endpoint(endpoint)
                        .err()
                        .map(ReturnedPublication::Native),
                    ReturnedPublication::Domain(pages) => {
                        if let Some(index) = pages.index_for(self.candidate_root.root_id())
                            && self.domain_pages[index].is_none()
                        {
                            self.domain_pages[index] = Some(pages);
                            None
                        } else {
                            Some(ReturnedPublication::Domain(pages))
                        }
                    }
                    ReturnedPublication::Candidate(value) => self
                        .candidate_root
                        .ingest(value)
                        .err()
                        .map(ReturnedPublication::Candidate),
                    ReturnedPublication::Eligible(value) => self
                        .candidate_root
                        .ingest_eligible(value)
                        .err()
                        .map(ReturnedPublication::Eligible),
                    // Handled above without holding any owner across destruction.
                    ReturnedPublication::Legacy(value) => Some(ReturnedPublication::Legacy(value)),
                };
                if let Some(value) = refused {
                    self.held_retirement = Some(value);
                    return false;
                }
            }
        }
        self.held_retirement.is_none() && self.publications.is_empty()
    }

    pub(super) fn return_graph(&mut self, publication: Publication) {
        self.return_graph_inner(publication, false);
    }

    /// Return a rejected pending graph and clear `publication_pending` before
    /// publishing its terminal receipt state so control threads observing the
    /// receipt never see a stale `publication_in_flight()` window (for example,
    /// when offering device-insert consent). Keeps the receipt owned inside
    /// `publication` so no `Arc` can drop on the audio callback thread.
    pub(super) fn return_rejected_graph(&mut self, publication: Publication) {
        self.return_graph_inner(publication, true);
    }

    fn return_graph_inner(&mut self, mut publication: Publication, clear_pending: bool) {
        if publication.approved_device_insert().is_some() {
            self.release_device_insert_fade_pins();
        }
        if (publication.approved_device_insert().is_some()
            || publication.tempo_cut().is_some()
            || publication.playback.native_removal_pending())
            && let Some(streams) = publication.streams.as_mut()
        {
            streams.cancel_install(&mut self.domain_pages);
        }
        self.release_refused_device_insert_cut(&publication);
        self.restore_refused_native_removal(&mut publication);
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        self.restore_parameter_predecessor_prewarm(&publication);
        if clear_pending {
            self.status.publication_pending.store(false, Release);
        }
        if let Some(receipt) = publication.fresh_runtime_cancellation.as_ref() {
            // The callback is the sole writer of these refusal fields. A
            // refused graph is returned in the same callback branch that
            // publishes the exact revision's reason.
            let refusal = self
                .status
                .rejected_revision()
                .filter(|(revision, _)| *revision == publication.revision)
                .map(|(_, reason)| reason);
            let closing = self.status.closing.load(Acquire);
            let disposition = refusal
                .or_else(|| (!closing).then_some(PlaybackHandoffRejection::RuntimeFenceChanged));
            if receipt.returned(disposition) && closing && refusal.is_none() {
                self.reject_publication(
                    publication.revision,
                    PlaybackHandoffRejection::FreshRuntimeCancelled,
                );
            }
        }
        self.return_publication(ReturnedPublication::Legacy(publication));
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) fn restore_parameter_predecessor_prewarm(&mut self, publication: &Publication) {
        let PublicationPolicy::InheritUnderNativeParameter { source, .. } = publication.policy
        else {
            return;
        };
        if self.status.closing.load(Acquire) {
            return;
        }
        if let Some(active) = self.active.as_mut().and_then(ActivePublication::legacy_mut)
            && active.revision == source.audio_revision
            && active.source_context == Some(source.context)
            && let Some(mut view) = active.stream_view()
        {
            view.publication_abandoned();
        }
    }
    pub(super) fn return_publication(&mut self, publication: ReturnedPublication) {
        if let ReturnedPublication::Domain(pages) = &publication {
            pages.departed_callback();
        }
        if let Err(PushError::Full(publication)) = self.retired.push(publication) {
            self.held_retirement = Some(publication);
        }
    }
    /// At most one replacement and 32 commands; no graph disposal or allocation.
    pub fn begin_block(&mut self) {
        self.begin_block_inner();
        self.flush_stream_outcomes();
    }
    pub(super) fn begin_block_inner(&mut self) {
        self.cold_device_fade = None;
        self.transport_setup_open = false;
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            self.paused_monitor_host_frames = None;
        }
        self.device_fault_fenced_in_block = false;
        let closing = self.status.closing.load(Acquire);
        let device_fault = self.status.device_fault.load(Acquire);
        self.input_monitor.begin_block(closing, device_fault);
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        self.paused_notes.begin_block(closing, device_fault);
        if self.status.device_fault.load(Acquire) && !self.status.closing.load(Acquire) {
            self.fence_device_fault();
            return;
        }
        if self.status.closing.load(Acquire) || self.status.pending_stop_marker.load(Acquire) != 0 {
            self.revoke_output_transport();
        }
        self.sample_preview
            .begin_block(self.status.closing.load(Acquire));
        if self.status.closing.load(Acquire) {
            self.invalidate_midi_recording(crate::midi_recording::AbortReason::Cancelled);
            self.invalidate_gain_recording(crate::gain_recording::AbortReason::Cancelled);
        }
        self.processor_audition.begin_block();
        self.apply_processor_audition_boundary();
        self.spectrum_target.begin_block();
        #[cfg(test)]
        if let Some(barrier) = self.test_after_spectrum_target_boundary.take() {
            barrier.wait();
            barrier.wait();
        }
        if let Some(publication) = self.held_retirement.take() {
            self.return_publication(publication);
            if self.held_retirement.is_some() {
                self.abort_blocked_verified_insert();
                self.abort_blocked_device_insert();
                // Ownership backpressure must not delay the independent Stop
                // path. Keep the graph retained while consuming bounded control.
                if self.status.closing.load(Acquire) {
                    self.status.playing.store(false, Relaxed);
                } else {
                    self.consume_commands();
                }
                return;
            }
        }
        self.return_requested_domain_pages();
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        self.native_sources.refresh_runtime_requirements(
            self.active
                .as_ref()
                .and_then(ActivePublication::legacy)
                .map(|active| active.playback.as_ref()),
        );
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        self.return_requested_native_sources();
        if self.status.closing.load(Acquire) {
            // A candidate return may end this block early; closing is already
            // silent and must expose stopped status at every such frontier.
            self.status.playing.store(false, Relaxed);
            self.release_device_insert_fade_pins();
            if !self.retired.is_full()
                && let Some(mut value) = self.deferred_eligible.take()
            {
                if value.receipt().is_some() {
                    value.cancel_stream_readiness(&mut self.domain_pages);
                }
                self.return_publication(ReturnedPublication::Eligible(
                    value.reject_before_install(),
                ));
                self.status.publication_pending.store(false, Release);
            }
            if !self.retired.is_full()
                && let Some(publication) = self.deferred.take()
            {
                self.return_rejected_graph(publication);
            }
            if !self.retired.is_full() {
                if let Some(mut active) = self
                    .active
                    .as_mut()
                    .and_then(ActivePublication::current_mut)
                {
                    active.playback.cancel_all_processor_auditions();
                }
                if matches!(self.active, Some(ActivePublication::Legacy(_)))
                    && let Some(ActivePublication::Legacy(publication)) = self.active.take()
                {
                    self.return_graph(publication);
                }
            }
            if !self.retired.is_full()
                && let Ok(publication) = self.publications.pop()
            {
                let publication = match publication {
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    QueuedPublication::NativeEpoch { .. } => return,
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    QueuedPublication::Native(endpoint) => {
                        self.return_publication(ReturnedPublication::Native(endpoint));
                        return;
                    }
                    QueuedPublication::Domain(pages) => {
                        self.return_publication(ReturnedPublication::Domain(pages));
                        return;
                    }
                    QueuedPublication::Eligible(value) => {
                        self.return_publication(ReturnedPublication::Eligible(
                            value.reject_before_install(),
                        ));
                        self.status.publication_pending.store(false, Release);
                        return;
                    }
                    QueuedPublication::Legacy(publication) => publication,
                    QueuedPublication::Candidate(candidate) => {
                        self.return_publication(ReturnedPublication::Candidate(
                            candidate.reject_before_install(),
                        ));
                        return;
                    }
                };
                self.return_rejected_graph(publication);
            }
            self.status.playing.store(false, Relaxed);
            if self.active.is_none()
                && self.published_route_key.is_some()
                && self.deferred.is_none()
                && self.deferred_eligible.is_none()
                && self.publications.is_empty()
            {
                self.status.publish_empty_routes();
                self.published_route_key = None;
            }
            if self.active.is_none()
                && self.deferred.is_none()
                && self.deferred_eligible.is_none()
                && self.publications.is_empty()
                && self.held_retirement.is_none()
                && self.sample_preview.is_empty()
            {
                self.status.shutdown_complete.store(true, Release);
            }
            return;
        }
        if self.deferred.is_none()
            && self.deferred_eligible.is_none()
            && (self.active.is_none() || !self.retired.is_full())
            && let Ok(publication) = self.publications.pop()
        {
            match publication {
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                QueuedPublication::NativeEpoch { target, fence } => {
                    self.consume_commands();
                    self.return_native_epoch_runtime(target, fence);
                    return;
                }
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                QueuedPublication::Native(endpoint) => {
                    if let Err(endpoint) = self.install_native_source_endpoint(endpoint) {
                        self.return_publication(ReturnedPublication::Native(endpoint));
                    }
                    self.consume_commands();
                    return;
                }
                QueuedPublication::Domain(pages) => {
                    if let Some(index) = pages.index_for(self.candidate_root.root_id())
                        && self.domain_pages[index].is_none()
                    {
                        self.domain_pages[index] = Some(pages);
                        if let Some(pages) = self.domain_pages[index].as_ref() {
                            pages.installed_on_callback();
                        }
                    } else {
                        self.return_publication(ReturnedPublication::Domain(pages));
                    }
                }
                QueuedPublication::Eligible(value) => {
                    self.deferred_eligible = Some(value);
                }
                QueuedPublication::Legacy(publication) => {
                    if matches!(
                        self.active,
                        Some(ActivePublication::Candidate(_) | ActivePublication::Eligible(_))
                    ) {
                        self.return_graph(publication);
                        self.consume_commands();
                        return;
                    }
                    self.deferred = Some(publication);
                }
                QueuedPublication::Candidate(candidate) => {
                    if let Err(candidate) = self.install_candidate(candidate) {
                        // Default production admission remains an unconditional
                        // refusal before active installation or render.
                        self.return_publication(ReturnedPublication::Candidate(
                            candidate.reject_before_install(),
                        ));
                    }
                    // Rejection must not starve priority Stop or the bounded
                    // command prefix belonging to the still-active graph.
                    self.consume_commands();
                    return;
                }
            }
        }

        if self.deferred_eligible.is_some() {
            self.advance_eligible_publication(None);
            self.consume_commands();
            return;
        }

        self.advance_legacy_publication(None);
        self.consume_commands();
    }

    pub(super) fn reject_publication(&mut self, revision: u64, reason: PlaybackHandoffRejection) {
        self.status.publish_rejected_revision(revision, reason);
        self.trace_audio(
            AudioDiagnosticKind::PublicationRejected { reason },
            revision,
            None,
            0,
        );
    }

    pub(super) fn trace_audio(
        &mut self,
        kind: AudioDiagnosticKind,
        revision: u64,
        context: Option<hex_engine::AudioSourceContext>,
        frames: usize,
    ) {
        if !self
            .status
            .audio_diagnostics_flags()
            .contains(kind.category())
        {
            return;
        }
        let event = AudioDiagnosticEvent {
            sequence: 0,
            output: self.candidate_root.root_id(),
            revision,
            context,
            applied_revision: self.status.applied_revision.load(Relaxed),
            consumed_command: self.status.consumed_command.load(Relaxed),
            performance_frame: self.status.performance_frame.load(Relaxed),
            arrangement_frame: self.status.position.load(Relaxed),
            rate: self.rate,
            frames,
            commands_pending: self.commands.slots(),
            retirement_slots: self.retired.slots(),
            kind,
        };
        self.diagnostics
            .push(event, &self.status.diagnostics_dropped);
    }
    /// Native adapter supplies measured host callback duration and entry cadence.
    /// Reading the clock and formatting remain outside this record operation.
    pub(crate) fn trace_callback_timing(
        &mut self,
        elapsed_ns: u64,
        deadline_ns: u64,
        frames: usize,
        start_interval_ns: Option<u64>,
    ) {
        self.trace_audio(
            AudioDiagnosticKind::CallbackTiming {
                elapsed_ns,
                deadline_ns,
                start_interval_ns,
            },
            self.status.applied_revision.load(Relaxed),
            None,
            frames,
        );
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) fn native_parameter_fence_matches(
        &self,
        source: AppliedAudioSource,
        transport_epoch: u64,
        playing: bool,
        successor: Option<hex_engine::AudioSourceContext>,
    ) -> bool {
        let predecessor = self.active.as_ref().and_then(ActivePublication::legacy);
        self.status.native_parameter_source(playing) == Some((source, transport_epoch))
            && source.output_identity == self.candidate_root.root_id()
            && predecessor.is_some_and(|active| {
                active.revision == source.audio_revision
                    && active.source_context == Some(source.context)
            })
            && successor.is_some_and(|context| {
                context.project_generation == source.context.project_generation
                    && source.context.editor_revision.checked_add(1)
                        == Some(context.editor_revision)
            })
    }

    pub(super) fn paused_input_graph_handoff(
        &self,
        next: &Publication,
    ) -> Option<(AppliedAudioSource, AppliedAudioSource, u64)> {
        let previous = self.active.as_ref()?.legacy()?;
        if !next.policy.inherits_runtime()
            || previous.playback.playing()
            || next.playback.sample_rate() != self.rate
            || self.status.pending_stop_marker.load(Acquire) != 0
        {
            return None;
        }
        let source = self.recording_source()?;
        let context = next.source_context?;
        if context.project_generation != source.context.project_generation {
            return None;
        }
        let successor = AppliedAudioSource {
            context,
            audio_revision: next.revision,
            output_identity: source.output_identity,
        };
        Some((source, successor, self.status.transport_epoch.load(Acquire)))
    }

    pub(super) fn runtime_fence_changed(
        status: &Status,
        active: Option<&Publication>,
        fence: FreshRuntimeFence,
    ) -> bool {
        let (revision, routes) = active.map_or((0, 0), |active| {
            (
                active.revision,
                active.playback.session_route_state_generation(),
            )
        });
        Self::runtime_frontier_changed(status, revision, routes, fence)
    }

    pub(super) fn runtime_frontier_changed(
        status: &Status,
        revision: u64,
        routes: u64,
        fence: FreshRuntimeFence,
    ) -> bool {
        revision != fence.expected_active_revision
            || routes != fence.route_state_generation
            || status.admitted_ticket.load(Acquire) != fence.admitted_command_ticket
            || status.last_output_fault_epoch.load(Acquire) != fence.output_fault_epoch
            || status.consumed_stop_generation.load(Acquire) != fence.consumed_stop_generation
            || status.render_fault.load(Acquire)
            || status.device_fault.load(Acquire)
            || status.pending_stop_marker.load(Acquire) != 0
    }

    pub(super) fn apply_processor_audition_boundary(&mut self) {
        let Some(boundary) = self.processor_audition.take_boundary() else {
            return;
        };
        if let Some(previous) = boundary.previous
            && let Some(mut active) = self
                .active
                .as_mut()
                .and_then(ActivePublication::current_mut)
        {
            if active.control_context_generation == previous.project_generation {
                if boundary.clear_mode == processor_audition::ClearMode::Immediate {
                    let _ = active.playback.cancel_processor_audition(previous.identity);
                } else {
                    let _ = active
                        .playback
                        .select_processor_audition(previous.identity, None);
                }
            } else if boundary.clear_mode == processor_audition::ClearMode::Immediate {
                active.playback.cancel_all_processor_auditions();
            }
        } else if boundary.clear_mode == processor_audition::ClearMode::Immediate
            && let Some(mut active) = self
                .active
                .as_mut()
                .and_then(ActivePublication::current_mut)
        {
            // A prior smooth release already cleared its lease target, but its
            // fade may still be audible. A later hard lifecycle cancel must
            // clear that residual transient state too.
            active.playback.cancel_all_processor_auditions();
        }
        self.processor_audition_lease = None;
        self.processor_audition_rejected_epoch = 0;
    }

    pub(super) fn reconcile_processor_audition(&mut self) {
        let epoch = self.processor_audition.current_epoch();
        let Some(target) = self.processor_audition.current() else {
            return;
        };
        if self.processor_audition_rejected_epoch == epoch {
            return;
        }
        let Some(mut active) = self
            .active
            .as_mut()
            .and_then(ActivePublication::current_mut)
        else {
            return;
        };
        if active.playback.sample_rate() != target.sample_rate_hz
            || target.sample_rate_hz != self.rate
        {
            active.playback.cancel_all_processor_auditions();
            self.processor_audition.reject_current(epoch);
            self.processor_audition_rejected_epoch = epoch;
            self.processor_audition_lease = None;
            return;
        }
        if active.control_context_generation < target.project_generation
            || active.revision < target.activation_floor
        {
            return;
        }
        if active.control_context_generation != target.project_generation {
            active.playback.cancel_all_processor_auditions();
            self.processor_audition.reject_current(epoch);
            self.processor_audition_rejected_epoch = epoch;
            self.processor_audition_lease = None;
            return;
        }
        if self.processor_audition_lease != Some(target) {
            if !active
                .playback
                .select_processor_audition(target.identity, Some(target.selection))
            {
                self.processor_audition.reject_current(epoch);
                self.processor_audition_rejected_epoch = epoch;
                self.processor_audition_lease = None;
                return;
            }
            self.processor_audition_lease = Some(target);
        }
        if !active
            .playback
            .processor_audition_is_active(target.identity, target.selection)
        {
            let _ = active.playback.cancel_processor_audition(target.identity);
            self.processor_audition.reject_current(epoch);
            self.processor_audition_rejected_epoch = epoch;
            self.processor_audition_lease = None;
        }
    }

    pub(super) fn consume_commands(&mut self) {
        if self.streamed_transport_mode() {
            self.consume_stream_commands();
            self.acknowledge_sampler_edit();
            return;
        }
        // Capture the published queue prefix before clearing the marker. Later
        // producer admissions stay queued for the next block; a Stop arriving
        // after this swap also remains pending for the next block.
        let entry_ticket = self.status.admitted_ticket.load(Acquire);
        let stop_marker = self.take_stop_marker();
        self.consume_commands_through(command_cut(entry_ticket, stop_marker));
        self.finish_stop_marker(stop_marker);
        self.acknowledge_sampler_edit();
    }

    // Publish the user Stop's recovery cursor before clearing its marker.
    // A racing newer marker stays pending for the next block; no callback retry.
    pub(super) fn take_stop_marker(&self) -> u64 {
        let marker = self.status.pending_stop_marker.load(Acquire);
        if marker != 0 {
            // This callback is the sole writer. The odd in-flight generation
            // is visible before clearing the marker, until Stop's transport
            // and route observations have been published. Saturation refuses
            // new fences permanently while priority Stop remains effective.
            let generation = self.status.consumed_stop_generation.load(Relaxed);
            self.status
                .consumed_stop_generation
                .store(generation.saturating_add(1), Release);
            // A reader may observe the cleared marker before Stop finishes.
            // Refuse any existing stopped witness before opening that window.
            self.status.sampler_edit.interrupt_for_stop();
            if !self.status.closing.load(Acquire) {
                self.status.recovery_position.store(0, Release);
            }
            let _ = self
                .status
                .pending_stop_marker
                .compare_exchange(marker, 0, AcqRel, Acquire);
        }
        marker
    }

    pub(super) fn finish_stop_marker(&self, marker: u64) {
        if marker != 0 {
            let generation = self.status.consumed_stop_generation.load(Relaxed);
            self.status
                .consumed_stop_generation
                .store(generation.saturating_add(1), Release);
        }
    }

    pub(super) fn trace_command_dequeued(&mut self, queued: QueuedCommand, cut: CommandCut) {
        self.trace_audio(
            AudioDiagnosticKind::CommandDequeued {
                ticket: queued.ticket,
                action: queued.action,
                discarded_by_stop: cut.stop_through.is_some_and(|stop| queued.ticket <= stop),
            },
            self.status.applied_revision.load(Relaxed),
            None,
            0,
        );
    }

    pub(super) fn consume_commands_through(&mut self, cut: CommandCut) {
        self.consume_commands_through_inner(cut, false);
    }

    pub(super) fn finish_action_outcome(&mut self, ticket: u64, outcome: action_outcomes::Outcome) {
        if let Some(writer) = self.action_writer.as_mut() {
            // A mismatch latches an observable failure and refuses further admission.
            let _ = writer.finish(ticket, outcome);
        }
    }

    pub(super) fn stage_stream_outcome(&mut self, ticket: u64, outcome: action_outcomes::Outcome) {
        let Some(writer) = self.action_writer.as_ref() else {
            return;
        };
        let Some(slot) = self.stream_outcomes.get_mut(self.stream_outcome_count) else {
            writer.fail_invariant();
            return;
        };
        *slot = Some((ticket, outcome));
        self.stream_outcome_count += 1;
    }

    pub(super) fn flush_stream_outcomes(&mut self) {
        if self.stream_outcome_count == 0 {
            return;
        }
        #[cfg(test)]
        if let Some(barrier) = self.test_before_action_outcome_status.take() {
            barrier.wait();
            barrier.wait();
        }
        self.publish_transport();
        self.publish_active_routes(false);
        let count = std::mem::take(&mut self.stream_outcome_count);
        for index in 0..count {
            if let Some((ticket, outcome)) = self.stream_outcomes[index].take() {
                self.finish_action_outcome(ticket, outcome);
            }
        }
    }

    pub(super) fn consume_commands_through_inner(&mut self, cut: CommandCut, mut fault: bool) {
        // A publication can complete a streamed terminal before handing the
        // same block to resident consumption. Preserve completion order.
        self.flush_stream_outcomes();
        if cut.stop_through.is_some() {
            // The immediate priority Stop follows an interrupted fade's effects.
            self.settle_transport_fade();
        }
        if let Some(through_ticket) = cut.stop_through {
            if !fault && !self.status.closing.load(Acquire) {
                self.recovery_position_frozen = false;
                self.status.recovery_position.store(0, Relaxed);
            }
            if !fault {
                self.trace_audio(
                    AudioDiagnosticKind::PriorityStop { through_ticket },
                    self.status.applied_revision.load(Relaxed),
                    None,
                    0,
                );
                self.finish_midi_before_stop();
                self.revoke_output_transport();
                self.revoke_current_sampler_keyboard();
                #[cfg(all(test, feature = "allocation-probe"))]
                self.apply_candidate_action(PlaybackAction::Transport(PlaybackCommand::Stop));
                self.invalidate_gain_recording(
                    crate::gain_recording::AbortReason::TransportChanged,
                );
                let mut priority_stop_failed = None;
                if let Some(mut active) = self
                    .active
                    .as_mut()
                    .and_then(ActivePublication::current_mut)
                {
                    let block_start = active.playback.position();
                    if active
                        .playback
                        .apply_priority_stop(&self.status.keyboard_activation)
                        .is_err()
                    {
                        priority_stop_failed = Some(block_start);
                    }
                }
                self.invalidate_processor_audition_request();
                if let Some(block_start) = priority_stop_failed {
                    self.report_render_fault(
                        Some(&Invalid("priority stop failed on active playback")),
                        block_start,
                    );
                    fault = true;
                }
            }
        }
        let mut consumed = None;
        let mut outcomes = [None; COMMANDS];
        for staged in &mut outcomes {
            if !self
                .commands
                .peek()
                .is_ok_and(|queued| queued.ticket <= cut.process_through)
            {
                break;
            }
            let Ok(queued) = self.commands.pop() else {
                break;
            };
            consumed = Some(queued.ticket);
            if !fault {
                self.trace_command_dequeued(queued, cut);
            }
            if !fault
                && cut.stop_through.is_none_or(|stop| queued.ticket > stop)
                && queued.project_generation.is_some_and(|generation| {
                    self.recording_source()
                        .is_none_or(|source| source.context.project_generation != generation)
                })
            {
                let reason = PlaybackActionRejection::ProjectChanged;
                self.status.publish_rejected_action(queued.ticket, reason);
                *staged = Some((queued.ticket, action_outcomes::Outcome::Rejected(reason)));
                continue;
            }
            if !fault
                && cut.stop_through.is_none_or(|stop| queued.ticket > stop)
                && queued
                    .expected_source
                    .is_some_and(|expected| self.recording_source() != Some(expected))
            {
                let reason = PlaybackActionRejection::AudioSourceChanged;
                self.status.publish_rejected_action(queued.ticket, reason);
                *staged = Some((queued.ticket, action_outcomes::Outcome::Rejected(reason)));
                continue;
            }
            // Each command follows the audible end of an earlier fade.
            self.settle_transport_fade();
            if fault && sampler_edit::is_stop(queued.action) {
                self.status.recovery_position.store(0, Release);
                *staged = Some((queued.ticket, action_outcomes::Outcome::Applied));
            } else if fault {
                self.status
                    .publish_rejected_action(queued.ticket, PlaybackActionRejection::OutputFailed);
                *staged = Some((
                    queued.ticket,
                    action_outcomes::Outcome::Rejected(PlaybackActionRejection::OutputFailed),
                ));
            }
            if fault {
                continue;
            }
            if stale_output_start(&self.status, queued) {
                self.status
                    .publish_rejected_action(queued.ticket, PlaybackActionRejection::OutputFailed);
                *staged = Some((
                    queued.ticket,
                    action_outcomes::Outcome::Rejected(PlaybackActionRejection::OutputFailed),
                ));
                continue;
            }
            if self.status.render_fault.load(Acquire) && starts_playback(queued.action) {
                if cut.stop_through.is_none_or(|stop| queued.ticket > stop)
                    && let Some(frame) = fault_cursor_intent(queued.action)
                {
                    self.status.recovery_position.store(frame, Release);
                    // An earlier applied Seek unfroze the cursor; keep this intent
                    // from being overwritten by the stopped graph's position.
                    self.recovery_position_frozen = true;
                }
                self.status
                    .publish_rejected_action(queued.ticket, PlaybackActionRejection::OutputFailed);
                *staged = Some((
                    queued.ticket,
                    action_outcomes::Outcome::Rejected(PlaybackActionRejection::OutputFailed),
                ));
                continue;
            }
            if !self.status.sampler_edit.permits(queued) {
                self.status.publish_rejected_action(
                    queued.ticket,
                    PlaybackActionRejection::SamplerEditBusy,
                );
                *staged = Some((
                    queued.ticket,
                    action_outcomes::Outcome::Rejected(PlaybackActionRejection::SamplerEditBusy),
                ));
                continue;
            }
            if cut
                .stop_through
                .is_none_or(|stop_through| queued.ticket > stop_through)
            {
                #[cfg(all(test, feature = "allocation-probe"))]
                self.apply_candidate_action(queued.action);
                let mut invalidate_audition = false;
                let mut fading = false;
                let outcome;
                let closing = self.status.closing.load(Acquire);
                if let Some(mut active) = self
                    .active
                    .as_mut()
                    .and_then(ActivePublication::current_mut)
                {
                    let applied = if closing {
                        active.playback.try_apply_action(queued.action)
                    } else {
                        active.playback.try_apply_declicked_action(queued.action)
                    };
                    match applied {
                        Ok(()) => {
                            fading =
                                active.playback.pending_transport_fade() == Some(queued.action);
                            outcome = action_outcomes::Outcome::Applied;
                            if recovery_cursor_action(queued.action) {
                                self.recovery_position_frozen = false;
                            }
                            invalidate_audition =
                                action_invalidates_processor_audition(queued.action);
                        }
                        Err(error) => {
                            let reason = error.telemetry_rejection();
                            self.status.publish_rejected_action(queued.ticket, reason);
                            outcome = action_outcomes::Outcome::Rejected(reason);
                        }
                    }
                } else {
                    self.status.publish_rejected_action(
                        queued.ticket,
                        PlaybackActionRejection::NoActivePlayback,
                    );
                    outcome = action_outcomes::Outcome::Rejected(
                        PlaybackActionRejection::NoActivePlayback,
                    );
                }
                *staged = Some((queued.ticket, outcome));
                let stop = matches!(
                    queued.action,
                    PlaybackAction::Transport(PlaybackCommand::Stop)
                );
                if fading {
                    // The output transport, epoch and sampler keyboard stay
                    // live until the fade's audible end.
                    self.begin_transport_fade(queued.ticket, queued.action, false);
                } else if stop {
                    self.revoke_current_sampler_keyboard();
                }
                if invalidate_audition
                    || matches!(
                        queued.action,
                        PlaybackAction::Transport(PlaybackCommand::Pause)
                            | PlaybackAction::Session(hex_engine::SessionPlaybackCommand::Pause)
                    )
                {
                    self.invalidate_midi_recording(
                        crate::midi_recording::AbortReason::TransportChanged,
                    );
                    if fading {
                        self.abort_transport_inputs(
                            crate::gain_recording::AbortReason::TransportChanged,
                        );
                    } else {
                        self.invalidate_for_transport_change(queued.action);
                    }
                }
                if invalidate_audition {
                    self.invalidate_processor_audition_request();
                }
            } else {
                *staged = Some((queued.ticket, action_outcomes::Outcome::Superseded));
            }
        }
        #[cfg(test)]
        if let Some(barrier) = self.test_before_action_outcome_status.take() {
            barrier.wait();
            barrier.wait();
        }
        self.publish_transport();
        self.publish_active_routes(false);
        if let Some(ticket) = consumed {
            self.status.consumed_command.store(ticket, Release);
        }
        for (ticket, outcome) in outcomes.into_iter().flatten() {
            self.finish_action_outcome(ticket, outcome);
        }
    }
    /// Latch the first actual render cause before Stop changes the cursor.
    /// None denotes nonfinite output after successful rendering.
    /// Native adapters call this on the callback; no formatting, I/O or ownership drops.
    pub fn report_render_fault(&mut self, error: Option<&Invalid>, block_start_frame: u64) {
        if !self.status.render_fault.load(Acquire) {
            self.status.render_fault_record.publish(
                error.map_or("audio output contains nonfinite samples", |error| error.0),
                self.status.applied_revision.load(Relaxed),
                block_start_frame,
            );
            self.status.render_fault.store(true, Release);
        }
        self.stop_after_fault();
    }
    /// Stop immediately after a render fault; does not dispose graph ownership.
    pub fn stop_after_fault(&mut self) {
        self.release_device_insert_fade_pins();
        if let Some(receipt) = self
            .deferred_eligible
            .as_ref()
            .and_then(|next| next.device_insert())
        {
            receipt.returned(None);
        }
        if let Some(next) = self.deferred.as_ref()
            && next.approved_device_insert().is_some()
            && let Some(receipt) = next.fresh_runtime_cancellation.as_ref()
        {
            receipt.returned(None);
        }
        // The fault Stop below revokes every transport effect immediately.
        self.transport_fade = None;
        self.recovery_position_frozen = true;
        if self.streamed_transport_mode() {
            self.cancel_stream_for_fault();
        }
        self.recovery_seek_pending = None;
        self.sample_preview.invalidate_on_fault();
        self.sample_recording
            .end(crate::sample_recording::End::OutputFailed);
        self.invalidate_midi_recording(crate::midi_recording::AbortReason::OutputFailed);
        self.revoke_output_transport();
        self.status.sampler_keyboard.revoke_on_callback();
        #[cfg(all(test, feature = "allocation-probe"))]
        self.apply_candidate_action(PlaybackAction::Transport(PlaybackCommand::Stop));
        self.invalidate_gain_recording(crate::gain_recording::AbortReason::OutputFailed);
        let mut fault_stop_failed = None;
        if let Some(mut active) = self
            .active
            .as_mut()
            .and_then(ActivePublication::current_mut)
        {
            let block_start = active.playback.position();
            if active.playback.apply(PlaybackCommand::Stop).is_err() {
                fault_stop_failed = Some(block_start);
            }
        }
        if let Some(block_start) = fault_stop_failed
            && !self.status.render_fault.load(Acquire)
        {
            self.status.render_fault_record.publish(
                "fault stop failed on active playback",
                self.status.applied_revision.load(Relaxed),
                block_start,
            );
            self.status.render_fault.store(true, Release);
        }
        self.processor_audition.invalidate_current_immediately();
        self.invalidate_processor_audition_request();
        self.publish_transport();
        self.publish_active_routes(false);
        self.flush_stream_outcomes();
    }

    // The internal cut is never published as a priority Stop: that marker is
    // reserved for the user's explicit cursor-reset intent. Drain at most one
    // bounded command prefix per callback and leave graph custody unchanged.
    pub(crate) fn fence_device_fault(&mut self) {
        if self.device_fault_fenced_in_block || self.status.closing.load(Acquire) {
            return;
        }
        self.device_fault_fenced_in_block = true;
        let entry = self.status.admitted_ticket.load(Acquire);
        let user_stop = self.take_stop_marker();
        if !self.device_fault_stopped {
            self.device_fault_stopped = true;
            self.stop_after_fault();
        } else {
            // A begin that raced the error still receives its reserved terminal.
            self.sample_recording
                .end(crate::sample_recording::End::OutputFailed);
            self.invalidate_midi_recording(crate::midi_recording::AbortReason::OutputFailed);
            self.gain_recording
                .abort(crate::gain_recording::AbortReason::OutputFailed);
            self.invalidate_native_parameter_input(
                crate::native_parameter_input::AbortReason::OutputFailed,
            );
        }
        let through = entry.max(user_stop.saturating_sub(1));
        let cut = CommandCut {
            stop_through: Some(through),
            process_through: through,
        };
        if self.streamed_transport_mode() {
            self.consume_stream_commands_through(cut, true);
        } else {
            self.consume_commands_through_inner(cut, true);
        }
        self.finish_stop_marker(user_stop);
        self.acknowledge_sampler_edit();
        self.flush_stream_outcomes();
    }

    pub(super) fn invalidate_processor_audition_request(&mut self) {
        if self.processor_audition.has_request_after_hard_cancel() {
            return;
        }
        let epoch = self.processor_audition.current_epoch();
        self.processor_audition.reject_current(epoch);
        self.processor_audition_rejected_epoch = epoch;
        self.processor_audition_lease = None;
        if let Some(mut active) = self
            .active
            .as_mut()
            .and_then(ActivePublication::current_mut)
        {
            active.playback.cancel_all_processor_auditions();
        }
    }
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
        self.publish_audible_position();
        self.flush_stream_outcomes();
        result.map(|_| ())
    }

    pub(super) fn record_audible_run(&mut self, block: Option<(u64, u64, bool)>, frames: usize) {
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
    pub(super) fn scheduled_transport(&self) -> Option<(u64, u64, bool)> {
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

    pub(super) fn publish_audible_position(&self) {
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
    pub(super) fn render_inner(
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
        if self.output_transport.is_some() {
            self.reject_paused_monitors(input_monitor::MonitorStatus::Unsupported);
            self.sample_recording
                .end(crate::sample_recording::End::UnsupportedOutput);
        } else {
            self.sample_recording.begin(output.len());
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
        self.report_sampler_keyboard_rejection();
        if !self.status.closing.load(Acquire) {
            self.sample_preview.mix(output);
        }
        Ok(RenderOutcome {
            audible_recorded: matches!(scheduled, section_launch::ScheduledCutRender::Switched),
            start: render_start,
        })
    }

    pub(super) fn render_project(
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
            let mut observe = |identity, phase, offset, frames: &[Frame]| {
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
        if !node_observed && !project_observed && recording.is_none() && monitor.is_none() {
            return self.render_unobserved(output, capture_offset);
        }
        if self.status.closing.load(Acquire) {
            output.fill([0.0; 2]);
            return Ok(());
        }
        let stages = &mut self.spectrum_stages;
        let targets = self.spectrum_target.current_targets();
        let mut observe = |identity, phase, offset, frames: &[Frame]| {
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
        let mut joined = [0; crate::sample_recording::TRACKS];
        let tracks =
            crate::sample_recording::merge_tracks(spectrum_tracks, record_tracks, &mut joined);
        // A failed selection merge must invalidate recording even when its
        // timing-only layout no longer relies on a Main PCM observation.
        if project_observed
            && tracks.is_none()
            && let Some((layout, stage)) = recording.as_mut()
        {
            stage.observe(layout, hex_engine::ProjectSignal::Unavailable, 0, &[]);
        }
        let recording_observed = recording.is_some();
        let timing_only_recording = recording
            .as_ref()
            .is_some_and(|(layout, _)| layout.timing_only);
        let recording = std::cell::RefCell::new(recording);
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
        };
        let mut observe_timing = |offset, start, frames, map: &hex_engine::PreparedTempoMap| {
            if let Ok(mut recording) = recording.try_borrow_mut()
                && let Some((_, stage)) = recording.as_mut()
            {
                stage.observe_timing(capture_offset + offset, start, frames, map);
            }
        };
        let mut project = if timing_only_recording && !project_observed {
            Some(hex_engine::ProjectOutputTap::timing_only(
                &mut observe_timing,
                &mut observe_project,
            ))
        } else {
            tracks
                .filter(|_| project_observed || recording_observed)
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
                active.playback.render_with_signal_observers(
                    output,
                    node,
                    project.as_mut(),
                    foreign,
                )?;
            }
        } else {
            output.fill([0.0; 2]);
        }
        self.publish_transport();
        self.publish_active_routes(false);
        Ok(())
    }

    pub(super) fn render_unobserved(
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
            active
                .playback
                .render_with_signal_observers(output, None, None, foreign)?;
        } else {
            output.fill([0.0; 2]);
        }
        self.publish_transport();
        self.publish_active_routes(false);
        Ok(())
    }

    pub(super) fn begin_project_spectrum(&mut self, frames: usize) {
        let selection = self.status.project_spectrum.read().filter(|s| s.active);
        let active = self.active.as_ref().and_then(ActivePublication::current);
        let Some(project) = self
            .spectrum_capture
            .as_mut()
            .and_then(|c| c.project.as_mut())
        else {
            return;
        };
        let valid = match (selection, active) {
            (Some(selection), Some(active))
                if selection.sample_rate_hz == self.rate
                    && active.playback.sample_rate() == self.rate
                    && active.control_context_generation == selection.generation
                    && active.revision >= selection.activation_floor =>
            {
                if project.needs_source_validation(selection, active.revision) {
                    if active.playback.project_output_supported(selection.tracks()) {
                        project.source_validated(selection, active.revision);
                        true
                    } else {
                        false
                    }
                } else {
                    true
                }
            }
            _ => false,
        };
        project.begin(selection, frames, valid);
    }
    pub(super) fn finish_project_spectrum(
        &mut self,
        captured_at: Instant,
        output_valid: bool,
    ) -> Option<crate::spectrum_worker::CaptureOutcome> {
        let selection = self
            .spectrum_capture
            .as_ref()
            .and_then(|c| c.project.as_ref())
            .and_then(|p| p.selection())?;
        let active = self.active.as_ref().and_then(ActivePublication::current);
        let revision = active.as_ref().map_or(0, |active| active.revision);
        let valid = output_valid
            && !self.status.closing.load(Acquire)
            && self.status.project_spectrum.token() == selection.token
            && selection.sample_rate_hz == self.rate
            && active.is_some_and(|active| {
                active.control_context_generation == selection.generation
                    && active.playback.sample_rate() == self.rate
                    && active.revision >= selection.activation_floor
            });
        self.spectrum_capture
            .as_mut()?
            .project
            .as_mut()?
            .finish(captured_at, revision, valid)
    }
    pub(super) fn finish_analysis_block(
        &mut self,
        captured_start: Instant,
        output_valid: bool,
    ) -> (
        Option<crate::spectrum_worker::CaptureOutcome>,
        Option<crate::spectrum_worker::CaptureOutcome>,
    ) {
        let project = self.finish_project_spectrum(captured_start, output_valid);
        let node = self.finish_eq_spectrum(captured_start, output_valid);
        (node, project)
    }

    /// Accept or reject the staged post-node signal after the caller has
    /// validated the complete final output block. A skipped attempt consumes a
    /// per-target sequence so the next accepted window cannot bridge the gap.
    /// Finalizes every target; the return value describes the first active slot
    /// for single-target callers. Native delivery does not depend on this value.
    pub fn finish_spectrum_block(
        &mut self,
        captured_start: Instant,
        output_valid: bool,
    ) -> Option<crate::spectrum_worker::CaptureOutcome> {
        self.finish_analysis_block(captured_start, output_valid).0
    }

    pub(crate) fn set_sample_recording_output_clock(
        &mut self,
        mut clock: Option<crate::sample_recording::OutputClock>,
    ) {
        if let Some(clock) = &mut clock {
            clock.graph_latency_frames = self
                .active
                .as_ref()
                .and_then(ActivePublication::current)
                .and_then(|active| active.playback.main_output_latency_frames());
        }
        self.sample_recording.set_output_clock(clock);
    }

    pub(crate) fn finish_sample_recording(&mut self, output_valid: bool) {
        let recording_source = self.recording_source();
        if output_valid
            && self.sample_recording.selection().is_some_and(|layout| {
                self.status.closing.load(Acquire)
                    || layout
                        .expected_source
                        .is_some_and(|source| Some(source) != recording_source)
                    || self
                        .active
                        .as_ref()
                        .and_then(ActivePublication::current)
                        .is_none_or(|active| {
                            active.control_context_generation != layout.generation
                                || !active.playback.project_sample_sources_supported(layout)
                        })
            })
        {
            self.sample_recording
                .end(crate::sample_recording::End::SourceChanged);
            return;
        }
        self.sample_recording.finish(output_valid);
    }
    pub(super) fn finish_eq_spectrum(
        &mut self,
        captured_start: Instant,
        output_valid: bool,
    ) -> Option<crate::spectrum_worker::CaptureOutcome> {
        let active = self
            .active
            .as_ref()
            .and_then(ActivePublication::current)
            .map(|active| {
                (
                    active.control_context_generation,
                    active.revision,
                    active.playback.sample_rate(),
                )
            });
        let mut first = None;
        for (slot, stage) in self.spectrum_stages.iter() {
            let Some(target) = stage.target else {
                continue;
            };
            let valid = output_valid
                && stage.complete()
                && self
                    .spectrum_target
                    .current_targets()
                    .and_then(|targets| targets.get(slot))
                    == Some(target)
                && self.spectrum_target.current_epoch() == stage.epoch
                && active.is_some_and(|(generation, revision, rate)| {
                    generation == target.project_generation
                        && revision >= target.activation_floor
                        && rate == target.sample_rate_hz
                })
                && target.sample_rate_hz == self.rate;
            let outcome = self.spectrum_capture.as_mut().map(|capture| {
                if valid {
                    capture.submit_pair_for_slot_at(
                        slot,
                        SpectrumStamp {
                            target: target.token,
                            generation: target.project_generation,
                            sample_rate_hz: target.sample_rate_hz,
                            applied_revision: active.map_or(0, |(_, revision, _)| revision),
                        },
                        &stage.input_frames[..stage.expected],
                        &stage.frames[..stage.expected],
                        captured_start,
                    )
                } else {
                    capture.skip_slot(slot)
                }
            });
            if first.is_none() {
                first = outcome;
            }
        }
        self.spectrum_stages.clear();
        first
    }

    pub(super) fn invalidate_gain_recording(&mut self, reason: crate::gain_recording::AbortReason) {
        self.revoke_native_effects();
        self.invalidate_transport_recording(reason);
    }

    pub(super) fn invalidate_for_transport_change(&mut self, action: PlaybackAction) {
        let reason = crate::gain_recording::AbortReason::TransportChanged;
        if is_pause_action(action) {
            // Pause retains the active graph for monitoring, release tails
            // and resume. The native endpoints belong to that same graph.
            self.invalidate_transport_recording(reason);
        } else {
            self.invalidate_gain_recording(reason);
        }
    }

    pub(super) fn invalidate_transport_recording(
        &mut self,
        reason: crate::gain_recording::AbortReason,
    ) {
        // Reuse the existing transport-discontinuity owner/epoch; asynchronous
        // output must not expose pre-seek/pause/stop buffers in a new context.
        self.revoke_forwarded_output_transport();
        self.abort_transport_inputs(reason);
        let prior_epoch = self
            .status
            .transport_epoch
            .fetch_update(AcqRel, Acquire, |epoch| epoch.checked_add(1));
        if matches!(reason, crate::gain_recording::AbortReason::OutputFailed) {
            self.status
                .last_output_fault_epoch
                .store(prior_epoch.unwrap_or_else(|epoch| epoch), Release);
        }
        if prior_epoch.is_err() {
            self.reject_paused_monitors(input_monitor::MonitorStatus::Stopped);
            self.status.gain_recording_fault.store(true, Release);
        }
    }
    /// Abort the gain take and native parameter gesture without revoking the
    /// output transport or advancing its epoch. A declicked Pause/Stop does
    /// this when consumed; the rest of `invalidate_gain_recording` follows at
    /// its fade end.
    pub(super) fn abort_transport_inputs(&mut self, reason: crate::gain_recording::AbortReason) {
        if matches!(reason, crate::gain_recording::AbortReason::TransportChanged) {
            self.sample_recording.end_bound_for_transport_change();
        }
        self.gain_recording.abort(reason);
        self.invalidate_native_parameter_input(match reason {
            crate::gain_recording::AbortReason::SourceChanged => {
                crate::native_parameter_input::AbortReason::SourceChanged
            }
            crate::gain_recording::AbortReason::TransportChanged => {
                crate::native_parameter_input::AbortReason::TransportChanged
            }
            crate::gain_recording::AbortReason::OutputFailed => {
                crate::native_parameter_input::AbortReason::OutputFailed
            }
            crate::gain_recording::AbortReason::Cancelled => {
                crate::native_parameter_input::AbortReason::Cancelled
            }
            crate::gain_recording::AbortReason::ClockExhausted => {
                crate::native_parameter_input::AbortReason::ClockExhausted
            }
        });
        if let Some(mut active) = self
            .active
            .as_mut()
            .and_then(ActivePublication::current_mut)
        {
            active.playback.clear_gain_recording_override();
        }
    }
    pub(super) fn invalidate_native_parameter_input(
        &mut self,
        reason: crate::native_parameter_input::AbortReason,
    ) {
        self.native_parameter_input.abort(reason);
        if let Some(mut active) = self
            .active
            .as_mut()
            .and_then(ActivePublication::current_mut)
        {
            active.playback.invalidate_native_parameter_input();
        }
    }
    pub(super) fn prepare_native_parameter_quantum(&mut self) {
        use crate::native_parameter_input::AbortReason;
        let source = self.status.recording_source();
        let transport_epoch = self.status.transport_epoch.load(Acquire);
        let unsupported_output = self.output_transport.is_some();
        let status = &self.status;
        let active = self
            .active
            .as_mut()
            .and_then(ActivePublication::current_mut);
        let had_input = self.native_parameter_input.has_input();
        let live = self.native_parameter_input.prepare(|target| {
            if status.render_fault.load(Acquire)
                || target.transport_epoch <= status.last_output_fault_epoch.load(Acquire)
            {
                return Err(AbortReason::OutputFailed);
            }
            if status.native_parameter_input_revoked(target.gesture_serial) {
                return Err(AbortReason::Cancelled);
            }
            if unsupported_output {
                return Err(AbortReason::UnsupportedOutput);
            }
            let active = active.as_ref().ok_or(AbortReason::SourceChanged)?;
            if source != Some(target.source)
                || active.revision != target.source.audio_revision
                || active.control_context_generation != target.source.context.project_generation
                || active.playback.native_parameter_input_target() != Some(target.input)
            {
                return Err(AbortReason::SourceChanged);
            }
            if transport_epoch != target.transport_epoch || !active.playback.playing() {
                return Err(AbortReason::TransportChanged);
            }
            Ok(())
        });
        if let Some(mut active) = active {
            if let Some(operation) = live {
                if active
                    .playback
                    .set_native_parameter_input(operation)
                    .is_err()
                {
                    self.native_parameter_input
                        .abort(AbortReason::TargetUnavailable);
                    active.playback.invalidate_native_parameter_input();
                }
            } else if had_input {
                active.playback.invalidate_native_parameter_input();
            }
        }
    }
    /// Final checked host write, not physical loudspeaker delivery. Both Set
    /// and Clear wait for the actual processed sample plus remaining Main delay.
    pub fn finish_native_parameter_quantum(&mut self, valid: bool) {
        use crate::native_parameter_input::AbortReason;
        let invalidated = self.native_parameter_input.owner().and_then(|target| {
            if self.status.closing.load(Acquire)
                || self
                    .status
                    .native_parameter_input_revoked(target.gesture_serial)
            {
                Some(AbortReason::Cancelled)
            } else if self.status.pending_stop_marker.load(Acquire) != 0
                || self.status.transport_epoch.load(Acquire) != target.transport_epoch
            {
                Some(AbortReason::TransportChanged)
            } else if self.status.recording_source() != Some(target.source) {
                Some(AbortReason::SourceChanged)
            } else {
                None
            }
        });
        if let Some(reason) = invalidated {
            self.invalidate_native_parameter_input(reason);
            return;
        }
        let cancelled = self.native_parameter_input.owner().is_some_and(|target| {
            self.status
                .native_parameter_input_revoked(target.gesture_serial)
        });
        let mut active = self
            .active
            .as_mut()
            .and_then(ActivePublication::current_mut);
        let application = active
            .as_mut()
            .and_then(|active| active.playback.take_native_parameter_application());
        let end = active
            .as_ref()
            .map_or(0, |active| active.playback.performance_frame());
        let had_input = self.native_parameter_input.has_input();
        let cleared =
            self.native_parameter_input
                .finish_quantum(application, end, valid, cancelled);
        if had_input
            && !self.native_parameter_input.has_input()
            && let Some(active) = &mut active
        {
            if cleared {
                active.playback.finish_native_parameter_input();
            } else {
                active.playback.invalidate_native_parameter_input();
            }
        }
        if self.native_parameter_input.receipt_fault {
            self.status
                .native_parameter_input_fault
                .store(true, Release);
        }
    }
    pub(super) fn prepare_gain_recording_quantum(&mut self) {
        use crate::gain_recording::{AbortReason, Clock};
        let transport_epoch = self.status.transport_epoch.load(Acquire);
        let status = &self.status;
        let active = self
            .active
            .as_mut()
            .and_then(ActivePublication::current_mut);
        let live = self.gain_recording.prepare(|target| {
            if status.render_fault.load(Acquire)
                || target.transport_epoch <= status.last_output_fault_epoch.load(Acquire)
            {
                return Err(AbortReason::OutputFailed);
            }
            let active = active.as_ref().ok_or(AbortReason::SourceChanged)?;
            if active.revision != target.audio_revision
                || active.control_context_generation != target.project_generation
                || active.playback.sample_rate() != target.sample_rate
                || !active.playback.gain_recording_ready(target.track_id)
            {
                return Err(AbortReason::SourceChanged);
            }
            if transport_epoch != target.transport_epoch || !active.playback.playing() {
                return Err(AbortReason::TransportChanged);
            }
            let (content_frame, delivery_delay, loop_pass) = active
                .playback
                .gain_recording_clock(target.track_id)
                .ok_or(AbortReason::SourceChanged)?;
            Ok(Clock {
                performance_frame: active.playback.performance_frame(),
                content_frame,
                delivery_delay,
                loop_pass,
            })
        });
        if let Some(mut active) = active
            && active.playback.set_gain_recording_input(live).is_err()
        {
            active.playback.clear_gain_recording_override();
            self.gain_recording.abort(AbortReason::SourceChanged);
        }
    }
    /// Confirm the exact converted output quantum; this is sink delivery, not
    /// physical loudspeaker playback. Failure discards the entire pending take.
    pub fn finish_gain_recording_quantum(&mut self, output_valid: bool) {
        if !output_valid
            && let Some(mut active) = self
                .active
                .as_mut()
                .and_then(ActivePublication::current_mut)
        {
            active.playback.clear_gain_recording_override();
        }
        let end = self
            .active
            .as_ref()
            .and_then(ActivePublication::current)
            .map_or(0, |active| active.playback.performance_frame());
        self.gain_recording.finish_quantum(end, output_valid);
        if self.gain_recording.receipt_fault {
            self.status.gain_recording_fault.store(true, Release);
        }
    }

    pub(super) fn publish_active_routes(&mut self, force: bool) {
        let Some(active) = self.active.as_ref().and_then(ActivePublication::current) else {
            return;
        };
        let revision = active.revision;
        let playback = &active.playback;
        let generation = playback.session_route_state_generation();
        let key = (revision, generation, playback.session_clock_anchor());
        if !force && self.published_route_key == Some(key) {
            return;
        }

        self.status
            .sampler_console
            .store(playback.sampler_console_available(), Release);
        let capability = playback.session_capability_status();
        self.status
            .session_capability
            .store(capability as u32, Relaxed);
        let count = if capability == SessionCapabilityStatus::Available {
            playback.session_route_count()
        } else {
            0
        };
        self.status.begin_route_snapshot();
        for index in 0..MAX_TRACKS {
            #[cfg(test)]
            {
                self.route_slot_visits = self.route_slot_visits.saturating_add(1);
            }
            if index < count {
                if let Some(route) = playback.session_route_status(index) {
                    self.status.publish_route_slot(index, route);
                } else if let Some(slot) = self.status.route_slots.get(index) {
                    slot.clear();
                }
            } else if let Some(slot) = self.status.route_slots.get(index) {
                slot.clear();
            }
        }
        self.status.publish_route_header(
            revision,
            count,
            capability,
            generation,
            playback.performance_frame(),
            playback.session_clock_anchor(),
        );
        self.status.end_route_snapshot();
        self.published_route_key = Some(key);
    }
    pub(super) fn publish_transport(&self) {
        let observation =
            self.active
                .as_ref()
                .and_then(ActivePublication::current)
                .and_then(|active| {
                    active.playback.transport().map(
                        |(position, performance, playing, loop_state)| TransportObservation {
                            audio_revision: active.revision,
                            control_generation: active.control_context_generation,
                            consumed_command_ticket: self.status.consumed_command.load(Acquire),
                            arrangement_frame: position,
                            performance_frame: performance,
                            playing,
                            loop_state,
                        },
                    )
                });
        self.status.transport_observation.publish(observation);
        self.status
            .recording_source
            .publish(self.recording_source());
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        let native_revision = self
            .native_sources
            .ordinary_runtime_requires_sources()
            .then(|| {
                self.active
                    .as_ref()
                    .and_then(ActivePublication::legacy)
                    .map(|active| active.revision)
            })
            .flatten()
            .unwrap_or(0);
        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        let native_revision = 0;
        self.status
            .ordinary_native_revision
            .store(native_revision, Release);
        #[cfg(all(test, feature = "allocation-probe"))]
        if let Some(ActivePublication::Candidate(active)) = self.active.as_ref() {
            if let Some((position, frame, playing)) = active.owner.test_transport() {
                self.status.position.store(position, Relaxed);
                if !self.recovery_position_frozen {
                    self.status.recovery_position.store(position, Relaxed);
                }
                self.status.performance_frame.store(frame, Relaxed);
                self.status.playing.store(playing, Relaxed);
            }
            return;
        }
        let ready =
            self.active
                .as_ref()
                .and_then(ActivePublication::current)
                .is_some_and(|active| {
                    active.playback.playing()
                        && active.playback.gain_recording_track().is_some_and(|track| {
                            active.playback.gain_recording_clock(track).is_some()
                        })
                });
        self.status.gain_recording_ready.store(ready, Release);
        if let Some(active) = self.active.as_ref().and_then(ActivePublication::current) {
            let position = active.playback.position();
            self.status.position.store(position, Relaxed);
            self.publish_audible_position();
            if !self.recovery_position_frozen {
                self.status.recovery_position.store(position, Relaxed);
            }
            self.status
                .performance_frame
                .store(active.playback.performance_frame(), Relaxed);
            self.status
                .playing
                .store(active.playback.playing(), Relaxed);
            self.status
                .metronome_enabled
                .store(active.playback.metronome_enabled(), Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callback_channel_enforces_sample_rate_bounds_and_storage_quote() {
        assert!(native_endpoint_bank_storage_quote().is_some());
        assert!(channel(0).is_err());
        assert!(channel(7_999).is_err());
        assert!(channel(192_001).is_err());
        assert!(channel(8_000).is_ok());
        assert!(channel(192_000).is_ok());
    }
}
