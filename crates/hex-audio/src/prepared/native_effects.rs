//! Physical callback acceptance for the original endpoint bank. Rendering only
//! stages input; the device calls finish after complete final-format conversion.
use super::*;
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
    pub(super) fn revoke_native_effects(&mut self) {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        if self.native_sources.has_endpoints() {
            self.native_sources.reject_rendering();
        }
    }
    pub(crate) fn begin_native_effect_callback(&mut self, frames: usize) -> Result<(), Invalid> {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
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
    pub(crate) fn finish_native_effect_callback(
        &mut self,
        accepted: bool,
    ) -> OutputTransportDelivery {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            if !self.native_sources.has_endpoints() {
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
    fn process_segment(
        &mut self,
        node: hex_lattice::ForeignProcessorNode<'_>,
        offset: usize,
        frames: &mut [[f32; 2]],
        clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
    ) -> Result<(), hex_lattice_contract::DeviceContractError> {
        self.process_segment_with_notes(node, offset, frames, &[], &[], clock)
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
    fn committed_tail_is_drained(&self, node: hex_lattice::ForeignProcessorNode<'_>) -> bool {
        self.inner.committed_tail_is_drained(node)
    }
    fn reject_callback(&mut self) {
        self.inner.reject_callback();
    }
}
