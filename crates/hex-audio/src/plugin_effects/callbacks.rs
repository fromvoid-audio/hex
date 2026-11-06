//! Bounded routing to exact already constructed endpoints. This owner is moved
//! through the existing callback carrier, never cloned or admitted by its IDs.
use super::*;

/// One original endpoint with its lazy staging bit. Both callback carriers use
/// this same processing/acceptance implementation; no worker custody is copied.
pub(crate) struct NativeEffectEntry {
    callback: NativeEffectCallback,
    staged: bool,
}
impl NativeEffectEntry {
    pub(crate) fn has_live_epoch(&self) -> bool {
        !self.staged && !self.callback.transport.is_revoked()
    }
    pub(crate) fn transport_is_live(&self) -> bool {
        !self.callback.transport.is_revoked()
    }
    pub(crate) fn new(callback: NativeEffectCallback) -> Self {
        Self {
            callback,
            staged: false,
        }
    }
    #[cfg(feature = "allocation-probe")]
    pub(crate) fn committed_frames_for_test(&self) -> u64 {
        self.callback.transport.committed_position_for_test()
    }
    pub(crate) fn callback(&self) -> &NativeEffectCallback {
        &self.callback
    }
    pub(crate) fn into_callback(self) -> NativeEffectCallback {
        self.callback
    }
    pub(crate) fn binding(&self) -> Binding {
        self.callback.binding()
    }
    pub(crate) fn quantum_frames(&self) -> usize {
        usize::from(self.callback.configuration.prepared.quantum_frames())
    }
    /// Render one visited segment. A fault of this endpoint's own transport
    /// (missed reply, invalid reply, worker/control revocation) is contained
    /// here (#3977): only this epoch is revoked, and it renders as an aligned
    /// pass-through placeholder for the rest of its life while every other
    /// endpoint keeps processing. A graph/configuration mismatch still fails
    /// the whole callback. Control observes the degraded entry through its
    /// worker's transport liveness; nothing is reported from the callback.
    #[allow(clippy::too_many_arguments)] // Borrow the existing graph segment and its clock.
    pub(crate) fn process(
        &mut self,
        extent: usize,
        node: ForeignProcessorNode<'_>,
        offset: usize,
        frames: &mut [[f32; 2]],
        parameters: &[hex_lattice_contract::ForeignParameterPoint],
        notes: &[ForeignNote],
        clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
    ) -> Result<(), DeviceContractError> {
        if !self.staged {
            if self.callback.begin_host_callback(extent).is_err() {
                self.callback.pass_through_revoked(node, frames);
                return Ok(());
            }
            self.staged = true;
        }
        match self
            .callback
            .process_segment_with_notes(node, offset, frames, parameters, notes, clock)
        {
            // Staging refusal already revoked this epoch and left aligned dry.
            Err(DeviceContractError::ForeignProcessingFailed) => {
                self.staged = false;
                Ok(())
            }
            result => result,
        }
    }
    /// Publish this endpoint's staged request after final host acceptance. A
    /// revocation or full request ring is this endpoint's own fault and is
    /// contained like a staging fault; a protocol breach fails the callback.
    pub(crate) fn finish(&mut self) -> Result<(), Fault> {
        if self.staged {
            self.staged = false;
            match self.callback.finish_host_callback(true) {
                Ok(()) | Err(Fault::Revoked | Fault::Saturated) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
    pub(crate) fn reject(&mut self) {
        self.callback.reject_callback();
        self.staged = false;
    }
}
/// Scalar state for one complete physical callback. An endpoint set's permanent
/// revocation is separate from which original entries were actually visited.
#[derive(Default)]
pub(crate) struct NativeEffectCycle {
    frames: Option<usize>,
    revoked: bool,
}
impl NativeEffectCycle {
    pub(crate) fn begin(&mut self, frames: usize, quantum: usize) -> Result<(), Fault> {
        if self.revoked || self.frames.is_some() || frames == 0 || frames > quantum {
            return Err(Fault::Protocol);
        }
        self.frames = Some(frames);
        Ok(())
    }
    pub(crate) fn extent(&self) -> Option<usize> {
        self.frames.filter(|_| !self.revoked)
    }
    pub(crate) fn revoked(&self) -> bool {
        self.revoked
    }
    pub(crate) fn active(&self) -> bool {
        self.frames.is_some()
    }
    pub(crate) fn finish(&mut self, accepted: bool) -> Result<(), Fault> {
        if !accepted || self.revoked || self.frames.take().is_none() {
            return Err(Fault::Protocol);
        }
        Ok(())
    }
    pub(crate) fn reject(&mut self) {
        self.revoked = true;
        self.frames = None;
    }
}

/// Current and retained-tail endpoints for one output. The sorted key includes
/// the original source graph and epoch: an Undo/replacement with the same device
/// ID cannot select an older native stream. Joining workers stay on control.
/// Construction does not admit resources to Root or authorize discontinuities.
pub struct NativeEffectCallbacks {
    entries: Vec<NativeEffectEntry>,
    quantum: usize,
    cycle: NativeEffectCycle,
}
impl NativeEffectCallbacks {
    /// Split actual constructor-paired owners on control. Returned workers use
    /// the same binding order as the callbacks and must outlive callback return.
    /// Different epochs of a device may coexist for current and retained tails;
    /// duplicate exact bindings and mixed output/rate/quantum refuse.
    pub fn from_effects_on_control(
        mut effects: Vec<NativeEffect>,
    ) -> Result<(Vec<NativeWorker>, Self), Error> {
        let first = effects
            .first()
            .ok_or(Error::Protocol("native effect endpoints are empty"))?;
        if effects.len() > hex_lattice::MAX_STATEFUL_PROCESSORS {
            return Err(Error::Protocol("native effect endpoint capacity exceeded"));
        }
        let root = first.binding().root;
        let rate = first.callback.configuration.sample_rate;
        let quantum = first.prepared().quantum_frames();
        effects.sort_unstable_by_key(NativeEffect::binding);
        if effects.windows(2).any(|w| w[0].binding() == w[1].binding())
            || effects.iter().any(|effect| {
                effect.binding().root != root
                    || effect.callback.configuration.sample_rate != rate
                    || effect.prepared().quantum_frames() != quantum
                    || !effect.callback.matches_worker(&effect.worker)
            })
        {
            return Err(Error::Protocol("native effect endpoint set differs"));
        }
        let mut workers = Vec::new();
        let mut entries = Vec::new();
        workers
            .try_reserve_exact(effects.len())
            .map_err(|_| Error::Protocol("native effect worker storage allocation refused"))?;
        entries
            .try_reserve_exact(effects.len())
            .map_err(|_| Error::Protocol("native effect endpoint storage allocation refused"))?;
        for effect in effects {
            let (worker, callback) = effect.into_parts();
            workers.push(worker);
            entries.push(NativeEffectEntry::new(callback));
        }
        Ok((
            workers,
            Self {
                entries,
                quantum: usize::from(quantum),
                cycle: NativeEffectCycle::default(),
            },
        ))
    }
    pub fn quantum_frames(&self) -> usize {
        self.quantum
    }
    /// Declare the complete physical callback. Endpoints begin only when the
    /// actual graph visits them, so stopped rendering or an unrelated retained
    /// source does not fabricate processing, silence input or advance its epoch.
    pub fn begin_host_callback(&mut self, frames: usize) -> Result<(), Fault> {
        let result = self.cycle.begin(frames, self.quantum);
        if result.is_err() {
            self.reject_callback();
        }
        result
    }
    /// Final host-format acceptance, once for the complete callback. Each used
    /// endpoint must have staged the full extent. A host refusal or protocol
    /// breach revokes every source; one endpoint's own transport fault revokes
    /// only that endpoint. Native work is irreversible and is never replayed.
    pub fn finish_host_callback(&mut self, accepted: bool) -> Result<(), Fault> {
        if let Err(error) = self.cycle.finish(accepted) {
            self.reject_callback();
            return Err(error);
        }
        for entry in &mut self.entries {
            if let Err(error) = entry.finish() {
                self.reject_callback();
                return Err(error);
            }
        }
        Ok(())
    }
}
impl ForeignProcessors for NativeEffectCallbacks {
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
        let selected = node.prepared.binding().and_then(|binding| {
            self.entries
                .binary_search_by_key(&binding, NativeEffectEntry::binding)
                .ok()
        });
        let result = match (self.cycle.extent(), selected) {
            (Some(extent), Some(index)) => {
                self.entries[index].process(extent, node, offset, frames, parameters, notes, clock)
            }
            _ => Err(DeviceContractError::ForeignProcessingUnavailable),
        };
        if result.is_err() {
            self.reject_callback();
            frames.fill([0.0; 2]);
        }
        result
    }
    fn committed_tail_is_drained(&self, node: ForeignProcessorNode<'_>) -> bool {
        !self.cycle.revoked()
            && node
                .prepared
                .binding()
                .and_then(|binding| {
                    self.entries
                        .binary_search_by_key(&binding, NativeEffectEntry::binding)
                        .ok()
                })
                .is_some_and(|index| {
                    self.entries[index]
                        .callback()
                        .committed_tail_is_drained(node)
                })
    }
    fn reject_callback(&mut self) {
        self.cycle.reject();
        for entry in &mut self.entries {
            entry.reject();
        }
    }
}

#[cfg(test)]
mod tests;
