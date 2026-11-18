//! Physical callback acceptance for the original endpoint bank. Rendering only
//! stages input; the device calls finish after complete final-format conversion.
use super::*;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use crate::candidate_custody::NativeEndpointBank;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use hex_lattice::{ForeignNote, ForeignProcessorNode, ForeignProcessors, NativeInstrumentClock};
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use hex_lattice_contract::{DeviceContractError, ForeignParameterPoint};

/// One physical render borrows original generators and private effects.
/// Neither bank accepts input here. The output owner accepts both after copy.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub(super) struct PausedNoteBanks<'a> {
    ordinary: &'a mut NativeEndpointBank,
    private: &'a mut NativeEndpointBank,
    frames: usize,
    staged_instrument: bool,
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
impl<'a> PausedNoteBanks<'a> {
    pub(super) fn new(
        ordinary: &'a mut NativeEndpointBank,
        private: &'a mut NativeEndpointBank,
        frames: usize,
    ) -> Self {
        Self {
            ordinary,
            private,
            frames,
            staged_instrument: false,
        }
    }

    pub(super) fn render(
        &mut self,
        scope: (AppliedAudioSource, u64),
        playback: &mut Playback,
        monitor: &mut hex_engine::PausedSourceMonitor,
        output: &mut [Frame],
        observer: Option<&mut hex_engine::NodeSignalObserver<'_>>,
    ) -> bool {
        if self.frames != output.len()
            || !self
                .private
                .matches_paused_monitor(scope, monitor.foreign_effect_bindings())
            || self.private.begin_rendering(self.frames).is_err()
        {
            return false;
        }
        if playback
            .render_paused_source_monitor_with_foreign(monitor, output, observer, self)
            .is_err()
            || !self.private.transports_are_live()
        {
            self.reject_callback();
            return false;
        }
        true
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
impl ForeignProcessors for PausedNoteBanks<'_> {
    fn returns_post_trim(&self, _: ForeignProcessorNode<'_>) -> bool {
        true
    }

    fn process_segment(
        &mut self,
        node: ForeignProcessorNode<'_>,
        offset: usize,
        frames: &mut [Frame],
        clock: Option<&dyn NativeInstrumentClock>,
    ) -> Result<(), DeviceContractError> {
        self.process_segment_with_notes(node, offset, frames, &[], &[], clock)
    }

    fn process_segment_with_notes(
        &mut self,
        node: ForeignProcessorNode<'_>,
        offset: usize,
        frames: &mut [Frame],
        parameters: &[ForeignParameterPoint],
        notes: &[ForeignNote],
        clock: Option<&dyn NativeInstrumentClock>,
    ) -> Result<(), DeviceContractError> {
        let bank = if node.prepared.is_instrument_profile() {
            self.staged_instrument = true;
            &mut self.ordinary
        } else {
            &mut self.private
        };
        bank.process_segment_with_notes(node, offset, frames, parameters, notes, clock)
    }

    fn committed_tail_is_drained(&self, node: ForeignProcessorNode<'_>) -> bool {
        let bank = if node.prepared.is_instrument_profile() {
            &self.ordinary
        } else {
            &self.private
        };
        bank.committed_tail_is_drained(node)
    }

    fn reject_callback(&mut self) {
        self.private.reject_rendering();
        if self.staged_instrument {
            self.ordinary.reject_rendering();
        }
    }
}

impl Callback {
    pub(crate) fn native_effect_quantum(&self) -> Option<usize> {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            self.native_sources.quantum_frames()
        }
        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        {
            None
        }
    }
    pub(super) fn native_effects_revoked(&self) -> bool {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            self.native_sources.rendering_revoked()
        }
        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        {
            false
        }
    }
    pub(crate) fn revoke_native_effects(&mut self) {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            if self.native_sources.has_endpoints() {
                self.native_sources.reject_rendering();
            }
            if self.paused_native_sources.has_endpoints() {
                self.paused_native_sources.reject_rendering();
            }
        }
    }
    pub(crate) fn begin_native_effect_callback(&mut self, frames: usize) -> Result<(), Invalid> {
        self.foreign_parameter_delivery.prepare();
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            self.paused_monitor_host_frames =
                self.paused_native_sources.has_endpoints().then_some(frames);
            if self.native_effects_revoked() && !self.status.playing.load(Relaxed) {
                return Ok(());
            }
            self.native_sources
                .begin_rendering(frames)
                .map_err(|_| Invalid("native effect physical callback refused"))
        }
        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        {
            let _ = frames;
            Ok(())
        }
    }
    pub(crate) fn finish_foreign_parameter_delivery(&mut self, valid: bool) {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        self.finish_foreign_editor_delivery(valid);
        let Some(target) = self.foreign_parameter_delivery.prepare_target() else {
            return;
        };
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        let marker = self
            .native_sources
            .take_accepted_manual_parameter(target.binding);
        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        let marker = None;
        // The typed admission excludes constructor and delayed-output paths.
        let active = self.active.as_ref().and_then(ActivePublication::legacy);
        let (end, latency) = active.map_or((0, 0), |active| {
            (
                active.playback.performance_frame(),
                active.playback.main_output_latency_frames(),
            )
        });
        self.foreign_parameter_delivery.finish(
            marker,
            self.status.live_native_parameter_source(),
            end,
            latency,
            valid,
            self.output_transport.is_none(),
        );
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn finish_foreign_editor_delivery(&mut self, valid: bool) {
        let Some(target) = self.foreign_editor_delivery.prepare_target() else {
            return;
        };
        let observed = self
            .native_sources
            .take_accepted_editor_quantum(target.binding);
        // This admission accepts only the direct, ordinary playing graph.
        let active = self.active.as_ref().and_then(ActivePublication::legacy);
        let (end, latency) = active.map_or((0, 0), |active| {
            (
                active.playback.performance_frame(),
                active.playback.main_output_latency_frames(),
            )
        });
        let source = self.status.live_native_parameter_source();
        let same_scope = source == Some((target.source, target.transport_epoch));
        // Priority Stop and source/epoch replacement can revoke the old
        // endpoint before this lookup. On a valid host block that is a scope
        // change, not an output fault. Earlier confirmed receipts stay queued.
        self.foreign_editor_delivery.finish(
            observed.ok().flatten(),
            source,
            end,
            latency,
            valid && (!same_scope || observed.is_ok()),
            self.output_transport.is_none(),
        );
    }

    pub(crate) fn finish_native_effect_callback(
        &mut self,
        accepted: bool,
    ) -> OutputTransportDelivery {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            self.paused_monitor_host_frames = None;
            if !self.has_native_source_endpoints() {
                return OutputTransportDelivery::Accepted;
            }
            // Priority Stop/close may arrive after conversion. Never dispatch
            // staged work or expose the copied old audio after this frontier.
            if self.status.closing.load(Acquire)
                || self.status.pending_stop_marker.load(Acquire) != 0
            {
                self.revoke_native_effects();
                return OutputTransportDelivery::Cancelled;
            }
            if self.native_effects_revoked() && !self.status.playing.load(Relaxed) {
                return OutputTransportDelivery::Cancelled;
            }
            if self.paused_native_sources.rendering_active() {
                let paused_notes = self.input_monitor.has_paused_source_binding();
                if paused_notes && !accepted {
                    self.revoke_native_effects();
                    self.input_monitor
                        .reject(input_monitor::MonitorStatus::OutputFault);
                    return OutputTransportDelivery::Cancelled;
                }
                // Ordinary input can contain borrowed generator notes for a
                // paused-source block. For physical input it was unvisited.
                if self.native_sources.finish_rendering(true).is_err() {
                    self.revoke_native_effects();
                    return OutputTransportDelivery::Refused;
                }
                if self
                    .paused_native_sources
                    .finish_rendering(accepted)
                    .is_err()
                    || !self.paused_native_sources.transports_are_live()
                {
                    if paused_notes {
                        self.native_sources.reject_rendering();
                    }
                    self.input_monitor
                        .reject(input_monitor::MonitorStatus::OutputFault);
                    return OutputTransportDelivery::Cancelled;
                }
                return OutputTransportDelivery::Accepted;
            }
            if self.native_sources.finish_rendering(accepted).is_ok() {
                OutputTransportDelivery::Accepted
            } else {
                // Other copied-transport work is irreversible too. A partial
                // acceptance failure revokes the entire output, never replays.
                self.revoke_output_transport();
                OutputTransportDelivery::Refused
            }
        }
        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        {
            let _ = accepted;
            OutputTransportDelivery::Accepted
        }
    }
}

/// A stream window borrows the same physical callback, preserving its absolute
/// offset for native input coverage across page/loop/route partitions.
pub(crate) struct NativeEffectWindow<'a> {
    start: usize,
    inner: &'a mut dyn hex_lattice::ForeignProcessors,
}
impl<'a> NativeEffectWindow<'a> {
    pub(crate) fn new(inner: &'a mut dyn hex_lattice::ForeignProcessors, start: usize) -> Self {
        Self { start, inner }
    }
}
impl hex_lattice::ForeignProcessors for NativeEffectWindow<'_> {
    fn returns_post_trim(&self, node: hex_lattice::ForeignProcessorNode<'_>) -> bool {
        self.inner.returns_post_trim(node)
    }
    fn process_segment(
        &mut self,
        node: hex_lattice::ForeignProcessorNode<'_>,
        offset: usize,
        frames: &mut [[f32; 2]],
        clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
    ) -> Result<(), hex_lattice_contract::DeviceContractError> {
        self.process_segment_with_notes(node, offset, frames, &[], &[], clock)
    }
    fn process_segment_with_parameters(
        &mut self,
        node: hex_lattice::ForeignProcessorNode<'_>,
        offset: usize,
        frames: &mut [Frame],
        parameters: &[hex_lattice_contract::ForeignParameterPoint],
        clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
    ) -> Result<(), hex_lattice_contract::DeviceContractError> {
        self.process_segment_with_notes(node, offset, frames, parameters, &[], clock)
    }
    fn process_segment_with_notes(
        &mut self,
        node: hex_lattice::ForeignProcessorNode<'_>,
        offset: usize,
        frames: &mut [[f32; 2]],
        parameters: &[hex_lattice_contract::ForeignParameterPoint],
        notes: &[hex_lattice::ForeignNote],
        clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
    ) -> Result<(), hex_lattice_contract::DeviceContractError> {
        let Some(offset) = self.start.checked_add(offset) else {
            self.inner.reject_callback();
            frames.fill([0.0; 2]);
            return Err(hex_lattice_contract::DeviceContractError::ForeignProcessingFailed);
        };
        self.inner
            .process_segment_with_notes(node, offset, frames, parameters, notes, clock)
    }
    fn process_segment_with_modulation(
        &mut self,
        node: hex_lattice::ForeignProcessorNode<'_>,
        offset: usize,
        frames: &mut [[f32; 2]],
        lane: hex_lattice::ForeignModulationSamples<'_>,
        clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
    ) -> Result<(), hex_lattice_contract::DeviceContractError> {
        let Some(offset) = self.start.checked_add(offset) else {
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
