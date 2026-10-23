//! Bounded routing to exact already constructed endpoints. This owner is moved
//! through the existing callback carrier, never cloned or admitted by its IDs.
use super::*;

struct Entry {
    callback: NativeEffectCallback,
    staged: bool,
}

/// Current and retained-tail endpoints for one output. The sorted key includes
/// the original source graph and epoch: an Undo/replacement with the same device
/// ID cannot select an older native stream. Joining workers stay on control.
/// Construction does not admit resources to Root or authorize discontinuities.
pub struct NativeEffectCallbacks {
    entries: Vec<Entry>,
    quantum: usize,
    frames: Option<usize>,
    revoked: bool,
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
            entries.push(Entry {
                callback,
                staged: false,
            });
        }
        Ok((
            workers,
            Self {
                entries,
                quantum: usize::from(quantum),
                frames: None,
                revoked: false,
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
        if self.revoked || self.frames.is_some() || frames == 0 || frames > self.quantum {
            self.reject_callback();
            return Err(Fault::Protocol);
        }
        self.frames = Some(frames);
        Ok(())
    }
    /// Final host-format acceptance, once for the complete callback. Each used
    /// endpoint must have staged the full extent. Any partial dispatch failure
    /// revokes every source; native work is irreversible and is never replayed.
    pub fn finish_host_callback(&mut self, accepted: bool) -> Result<(), Fault> {
        if !accepted || self.revoked || self.frames.take().is_none() {
            self.reject_callback();
            return Err(Fault::Protocol);
        }
        for entry in &mut self.entries {
            if entry.staged {
                if let Err(error) = entry.callback.finish_host_callback(true) {
                    self.reject_callback();
                    return Err(error);
                }
                entry.staged = false;
            }
        }
        Ok(())
    }
}
impl ForeignProcessors for NativeEffectCallbacks {
    fn process_segment(
        &mut self,
        node: ForeignProcessorNode<'_>,
        offset: usize,
        frames: &mut [[f32; 2]],
        clock: Option<&dyn hex_lattice::NativeInstrumentClock>,
    ) -> Result<(), DeviceContractError> {
        let selected = node.prepared.binding().and_then(|binding| {
            self.entries
                .binary_search_by_key(&binding, |entry| entry.callback.binding())
                .ok()
        });
        let result = match (self.frames, selected) {
            (Some(extent), Some(index)) if !self.revoked => {
                let entry = &mut self.entries[index];
                if entry.staged || entry.callback.begin_host_callback(extent).is_ok() {
                    entry.staged = true;
                    entry.callback.process_segment(node, offset, frames, clock)
                } else {
                    Err(DeviceContractError::ForeignProcessingFailed)
                }
            }
            _ => Err(DeviceContractError::ForeignProcessingUnavailable),
        };
        if result.is_err() {
            self.reject_callback();
            frames.fill([0.0; 2]);
        }
        result
    }
    fn reject_callback(&mut self) {
        self.revoked = true;
        self.frames = None;
        for entry in &mut self.entries {
            entry.callback.reject_callback();
        }
    }
}
