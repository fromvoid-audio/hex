//! `Callback` struct definition, storage quotes, channel constructors, and output transport setup.
use crate::prepared::*;

pub struct Callback {
    pub(in crate::prepared) host_callback_entry: Option<Instant>,
    pub(in crate::prepared) diagnostics: audio_diagnostics::TraceWriter,
    pub(in crate::prepared) action_writer: Option<action_outcomes::Writer>,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(in crate::prepared) native_sources: crate::candidate_custody::NativeEndpointBank,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(in crate::prepared) paused_native_sources: crate::candidate_custody::NativeEndpointBank,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(in crate::prepared) paused_monitor_host_frames: Option<usize>,
    pub(in crate::prepared) plugin_callback_quantum: Option<u16>,
    pub(in crate::prepared) output_transport: Option<Box<hex_plugin_host::transport::Callback>>,
    pub(in crate::prepared) output_transport_cancelled: bool,
    pub(in crate::prepared) transport_setup_open: bool,
    pub(in crate::prepared) sample_preview: crate::sample_preview::Callback,
    pub(in crate::prepared) sample_recording: crate::sample_recording::Callback,
    pub(in crate::prepared) input_monitor: input_monitor::Callback,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(in crate::prepared) paused_notes: input_monitor::Callback,
    pub(in crate::prepared) publications: Consumer<QueuedPublication>,
    pub(in crate::prepared) retired: Producer<ReturnedPublication>,
    pub(in crate::prepared) candidate_root: crate::candidate_custody::CandidateRoot,
    pub(in crate::prepared) commands: Consumer<QueuedCommand>,
    pub(in crate::prepared) status: Arc<Status>,
    pub(in crate::prepared) audible_clock: audible_clock::AudibleClock,
    pub(in crate::prepared) recovery_position_frozen: bool,
    pub(in crate::prepared) recovery_seek_pending: Option<u64>,
    pub(in crate::prepared) device_fault_stopped: bool,
    pub(in crate::prepared) device_fault_fenced_in_block: bool,
    pub(in crate::prepared) rate: u32,
    pub(in crate::prepared) spectrum_target: spectrum_target::SpectrumTargetCallback,
    pub(in crate::prepared) processor_audition: processor_audition::ProcessorAuditionTargetCallback,
    pub(in crate::prepared) processor_audition_lease:
        Option<processor_audition::ProcessorAuditionTarget>,
    pub(in crate::prepared) processor_audition_rejected_epoch: u64,
    pub(in crate::prepared) gain_recording: crate::gain_recording::Callback,
    pub(in crate::prepared) native_parameter_input: Box<crate::native_parameter_input::Callback>,
    pub(in crate::prepared) foreign_parameter_delivery: crate::foreign_parameter_delivery::Callback,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(in crate::prepared) foreign_editor_delivery:
        crate::foreign_parameter_delivery::editor::Callback,
    pub(in crate::prepared) midi_recording: crate::midi_recording::Callback,
    pub(in crate::prepared) spectrum_capture: Option<SpectrumCapture>,
    pub(in crate::prepared) spectrum_stages: SpectrumStages,
    pub(in crate::prepared) active: Option<ActivePublication>,
    #[cfg(test)]
    pub(in crate::prepared) test_admit_candidate: bool,
    pub(in crate::prepared) deferred: Option<Publication>,
    pub(in crate::prepared) deferred_eligible:
        Option<crate::candidate_custody::QueuedEligibleCandidate>,
    pub(in crate::prepared) domain_pages: [Option<crate::candidate_custody::DomainEnvelope>; 2],
    pub(in crate::prepared) stream_pending_play: Option<u64>,
    // Credits remain reserved until publication, so no more than the ledger
    // capacity can accumulate before this callback publishes and flushes them.
    pub(in crate::prepared) stream_outcomes:
        [Option<(u64, action_outcomes::Outcome)>; ACTION_OUTCOME_CAPACITY],
    pub(in crate::prepared) stream_outcome_count: usize,
    pub(in crate::prepared) transport_fade: Option<transport_fade::PendingFade>,
    pub(in crate::prepared) yield_stream_successor_once: bool,
    pub(in crate::prepared) current_quantum_pins: Option<crate::candidate_custody::QuantumPins>,
    pub(in crate::prepared) device_insert_fade_pins: Option<crate::candidate_custody::QuantumPins>,
    pub(in crate::prepared) cold_device_fade: Option<device_insert_consent::ColdDeviceFade>,
    pub(in crate::prepared) stream_prefix: u64,
    // Defensive retention if a push unexpectedly fails; never drop graph ownership.
    pub(in crate::prepared) held_retirement: Option<ReturnedPublication>,
    pub(in crate::prepared) terminal_disposal_pending: bool,
    pub(in crate::prepared) published_route_key:
        Option<(u64, u64, Option<hex_engine::SessionClockAnchor>)>,
    #[cfg(test)]
    pub(in crate::prepared) route_slot_visits: usize,
    #[cfg(test)]
    pub(in crate::prepared) test_transient_handoff_refusals: usize,
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(in crate::prepared) test_eligible_transfer_allowance: Option<usize>,
    #[cfg(test)]
    pub(in crate::prepared) test_after_fresh_fence_validation:
        Option<std::sync::Arc<std::sync::Barrier>>,
    #[cfg(test)]
    pub(in crate::prepared) test_before_fresh_runtime_apply:
        Option<std::sync::Arc<std::sync::Barrier>>,
    #[cfg(test)]
    pub(in crate::prepared) test_after_fresh_runtime_apply_decision:
        Option<std::sync::Arc<std::sync::Barrier>>,
    #[cfg(test)]
    pub(in crate::prepared) test_after_spectrum_target_boundary:
        Option<std::sync::Arc<std::sync::Barrier>>,
    #[cfg(test)]
    pub(in crate::prepared) test_before_action_outcome_status:
        Option<std::sync::Arc<std::sync::Barrier>>,
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
            host_callback_entry: None,
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
    pub(in crate::prepared) fn reject_paused_monitors(
        &mut self,
        reason: input_monitor::MonitorStatus,
    ) {
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

    pub(in crate::prepared) fn revoke_forwarded_output_transport(&mut self) {
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
}
