//! Borrowed rendering through the actual loaned endpoint bank. Lazy staging and
//! final acceptance use the same entry/cycle implementation as NativeEffectCallbacks.
use super::*;
use hex_lattice::{ForeignModulationSamples, ForeignNote, ForeignProcessorNode, ForeignProcessors};
use hex_lattice_contract::DeviceContractError;
use hex_plugin_host::transport::Fault;
impl NativeEndpointBank {
    #[cfg(feature = "allocation-probe")]
    pub(crate) fn test_transport_fault(&self, binding: Binding) -> Option<Fault> {
        self.entries
            .iter()
            .flatten()
            .find(|entry| entry.0.binding == binding)?
            .0
            .callback
            .as_ref()?
            .callback()
            .transport_fault_for_test()
    }
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(crate) fn test_pending_requests(&self, binding: Binding) -> Option<usize> {
        Some(
            self.entries
                .iter()
                .flatten()
                .find(|entry| entry.0.binding == binding)?
                .0
                .callback
                .as_ref()?
                .callback()
                .pending_requests_for_test(),
        )
    }
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(crate) fn test_committed_frames(&self, binding: Binding) -> Option<u64> {
        self.entries
            .iter()
            .flatten()
            .find(|e| e.0.binding == binding)?
            .0
            .callback
            .as_ref()
            .map(NativeEffectEntry::committed_frames_for_test)
    }
    pub(crate) fn quantum_frames(&self) -> Option<usize> {
        self.routing[..self.routing_len]
            .iter()
            .filter_map(|slot| self.entries[*slot].as_ref()?.0.callback.as_ref())
            .map(NativeEffectEntry::quantum_frames)
            .min()
    }
    pub(crate) fn rendering_revoked(&self) -> bool {
        self.has_endpoints() && self.cycle.revoked()
    }
    pub(crate) fn begin_rendering(&mut self, frames: usize) -> Result<(), Fault> {
        let Some(quantum) = self.quantum_frames() else {
            return Ok(());
        };
        let result = self.cycle.begin(frames, quantum);
        if result.is_err() {
            self.reject_rendering();
        }
        result
    }
    pub(crate) fn finish_rendering(&mut self, accepted: bool) -> Result<(), Fault> {
        if !self.has_endpoints() {
            return Ok(());
        }
        if let Err(error) = self.cycle.finish(accepted) {
            self.reject_rendering();
            return Err(error);
        }
        for index in 0..self.routing_len {
            if let Some(entry) = self.entries[self.routing[index]]
                .as_mut()
                .and_then(|e| e.0.callback.as_mut())
                && let Err(error) = entry.finish()
            {
                self.reject_rendering();
                return Err(error);
            }
        }
        Ok(())
    }
    pub(crate) fn end_staging_at_rendered_prefix(&mut self, frame: usize) {
        for index in 0..self.routing_len {
            if let Some(entry) = self.entries[self.routing[index]]
                .as_mut()
                .and_then(|entry| entry.0.callback.as_mut())
            {
                entry.end_staging_at_rendered_prefix(frame);
            }
        }
    }
    /// A full physical render can commit its streamed prefix and hold the
    /// remaining output silent. Split renders may still visit a later span.
    pub(crate) fn end_staging_at_stream_hold(
        &mut self,
        offset: usize,
        frames: usize,
        committed_frames: usize,
    ) {
        if offset == 0 && self.cycle.extent() == Some(frames) {
            self.end_staging_at_rendered_prefix(committed_frames);
        }
    }
    pub(crate) fn reject_rendering(&mut self) {
        self.cycle.reject();
        for index in 0..self.routing_len {
            if let Some(entry) = self.entries[self.routing[index]]
                .as_mut()
                .and_then(|e| e.0.callback.as_mut())
            {
                entry.reject();
            }
        }
    }
}
impl NativeEndpointBank {
    #[allow(clippy::too_many_arguments)] // Borrowed views share one physical acceptance.
    fn stage_segment(
        &mut self,
        node: ForeignProcessorNode<'_>,
        offset: usize,
        frames: &mut [[f32; 2]],
        parameters: &[hex_lattice_contract::ForeignParameterPoint],
        notes: &[ForeignNote],
        clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
        lane: Option<ForeignModulationSamples<'_>>,
    ) -> Result<(), DeviceContractError> {
        let selected = node.prepared.binding().and_then(|binding| {
            self.routing[..self.routing_len]
                .binary_search_by_key(&Some(binding), |slot| {
                    self.entries[*slot].as_ref().map(|e| e.0.binding)
                })
                .ok()
                .map(|index| self.routing[index])
        });
        let result = match (self.cycle.extent(), selected) {
            (Some(extent), Some(index)) => self.entries[index]
                .as_mut()
                .and_then(|e| e.0.callback.as_mut())
                .ok_or(DeviceContractError::ForeignProcessingUnavailable)
                .and_then(|entry| {
                    entry.process(extent, node, offset, frames, parameters, notes, clock, lane)
                }),
            _ => Err(DeviceContractError::ForeignProcessingUnavailable),
        };
        if result.is_err() {
            self.reject_rendering();
            frames.fill([0.0; 2]);
        }
        result
    }
}
impl ForeignProcessors for NativeEndpointBank {
    fn returns_post_trim(&self, _node: ForeignProcessorNode<'_>) -> bool {
        true
    }
    fn process_segment(
        &mut self,
        node: ForeignProcessorNode<'_>,
        offset: usize,
        frames: &mut [[f32; 2]],
        clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
    ) -> Result<(), DeviceContractError> {
        self.process_segment_with_parameters(node, offset, frames, &[], clock)
    }
    fn process_segment_with_parameters(
        &mut self,
        node: ForeignProcessorNode<'_>,
        offset: usize,
        frames: &mut [[f32; 2]],
        parameters: &[hex_lattice_contract::ForeignParameterPoint],
        clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
    ) -> Result<(), DeviceContractError> {
        self.process_segment_with_notes(node, offset, frames, parameters, &[], clock)
    }
    fn process_segment_with_notes(
        &mut self,
        node: ForeignProcessorNode<'_>,
        offset: usize,
        frames: &mut [[f32; 2]],
        parameters: &[hex_lattice_contract::ForeignParameterPoint],
        notes: &[ForeignNote],
        clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
    ) -> Result<(), DeviceContractError> {
        self.stage_segment(node, offset, frames, parameters, notes, clock, None)
    }
    fn process_segment_with_modulation(
        &mut self,
        node: ForeignProcessorNode<'_>,
        offset: usize,
        frames: &mut [[f32; 2]],
        lane: ForeignModulationSamples<'_>,
        clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
    ) -> Result<(), DeviceContractError> {
        self.stage_segment(node, offset, frames, &[], &[], clock, Some(lane))
    }
    fn committed_tail_is_drained(&self, node: ForeignProcessorNode<'_>) -> bool {
        !self.cycle.revoked()
            && node
                .prepared
                .binding()
                .and_then(|binding| {
                    self.routing[..self.routing_len]
                        .binary_search_by_key(&Some(binding), |slot| {
                            self.entries[*slot].as_ref().map(|entry| entry.0.binding)
                        })
                        .ok()
                        .map(|index| self.routing[index])
                })
                .and_then(|index| self.entries[index].as_ref()?.0.callback.as_ref())
                .is_some_and(|entry| entry.callback().committed_tail_is_drained(node))
    }
    fn reject_callback(&mut self) {
        self.reject_rendering();
    }
}
