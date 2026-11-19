//! Callback-local fixed-window staging. The control constructor allocates all
//! 64 slots; render/finish only borrow them. Stable target slots and binary
//! identity lookup avoid a node-by-target scan. Heap payload belongs to the
//! callback owner and is reclaimed on control after callback quiescence.
use super::*;
use spectrum_target::{MAX_SPECTRUM_TARGETS, SpectrumTargets};

pub(super) struct SpectrumStage {
    pub(super) frames: [[f32; 2]; 256],
    pub(super) input_frames: [[f32; 2]; 256],
    pub(super) seen: [u64; 4],
    pub(super) input_seen: [u64; 4],
    pub(super) expected: usize,
    pub(super) observed: bool,
    pub(super) valid: bool,
    pub(super) target: Option<spectrum_target::SpectrumTarget>,
    pub(super) epoch: u64,
}

impl Default for SpectrumStage {
    fn default() -> Self {
        Self {
            frames: [[0.0; 2]; 256],
            input_frames: [[0.0; 2]; 256],
            seen: [0; 4],
            input_seen: [0; 4],
            expected: 0,
            observed: false,
            valid: false,
            target: None,
            epoch: 0,
        }
    }
}

impl SpectrumStage {
    pub(super) fn begin(
        &mut self,
        target: spectrum_target::SpectrumTarget,
        epoch: u64,
        expected: usize,
    ) {
        self.frames.fill([0.0; 2]);
        self.input_frames.fill([0.0; 2]);
        self.seen.fill(0);
        self.input_seen.fill(0);
        self.expected = expected;
        self.observed = false;
        self.valid = expected <= self.frames.len();
        self.target = Some(target);
        self.epoch = epoch;
    }

    pub(super) fn observe_signal(
        &mut self,
        identity: PostNodeIdentity,
        phase: hex_engine::NodeSignalPhase,
        offset: usize,
        frames: &[[f32; 2]],
    ) {
        let Some(target) = self.target else {
            return;
        };
        if identity != target.identity {
            return;
        }
        self.observed = true;
        let Some(end) = offset.checked_add(frames.len()) else {
            self.valid = false;
            return;
        };
        if end > self.expected || end > self.frames.len() {
            self.valid = false;
            return;
        }
        let (samples, seen) = match phase {
            hex_engine::NodeSignalPhase::Input => (&mut self.input_frames, &mut self.input_seen),
            hex_engine::NodeSignalPhase::Output => (&mut self.frames, &mut self.seen),
        };
        for (relative, frame) in frames.iter().enumerate() {
            let index = offset + relative;
            let word = index / 64;
            let mask = 1_u64 << (index % 64);
            let Some(bits) = seen.get_mut(word) else {
                self.valid = false;
                return;
            };
            if *bits & mask != 0 {
                self.valid = false;
                return;
            }
            *bits |= mask;
            samples[index] = *frame;
        }
    }

    pub(super) fn complete(&self) -> bool {
        self.valid
            && self.observed
            && self.target.is_some()
            && (0..self.expected).all(|index| {
                let mask = 1_u64 << (index % 64);
                self.seen[index / 64] & mask != 0 && self.input_seen[index / 64] & mask != 0
            })
    }

    pub(super) fn clear(&mut self) {
        self.target = None;
        self.expected = 0;
        self.observed = false;
        self.valid = false;
        self.seen.fill(0);
        self.input_seen.fill(0);
    }
}

pub(super) struct SpectrumStages {
    slots: Box<[SpectrumStage]>,
    active: [usize; MAX_SPECTRUM_TARGETS],
    len: usize,
}
impl SpectrumStages {
    pub(super) fn new() -> Result<Self, Invalid> {
        let mut slots = Vec::new();
        slots
            .try_reserve_exact(MAX_SPECTRUM_TARGETS)
            .map_err(|_| Invalid("could not reserve spectrum capture stages"))?;
        slots.resize_with(MAX_SPECTRUM_TARGETS, SpectrumStage::default);
        Ok(Self {
            slots: slots.into_boxed_slice(),
            active: [0; MAX_SPECTRUM_TARGETS],
            len: 0,
        })
    }
    pub(super) fn heap_data_bytes(&self) -> Option<usize> {
        (self.slots.len() == MAX_SPECTRUM_TARGETS)
            .then(|| std::mem::size_of_val(self.slots.as_ref()))
    }
    pub(super) fn storage_quote() -> Option<usize> {
        MAX_SPECTRUM_TARGETS.checked_mul(std::mem::size_of::<SpectrumStage>())
    }
    pub(super) fn begin(
        &mut self,
        targets: Option<&SpectrumTargets>,
        epoch: u64,
        expected: usize,
        mut valid: impl FnMut(spectrum_target::SpectrumTarget) -> bool,
    ) {
        self.clear();
        for (slot, target) in targets.into_iter().flat_map(SpectrumTargets::iter) {
            if let (Some(stage), Some(active)) =
                (self.slots.get_mut(slot), self.active.get_mut(self.len))
            {
                stage.begin(target, epoch, expected);
                stage.valid &= valid(target);
                *active = slot;
                self.len += 1;
            }
        }
    }
    pub(super) fn iter(&self) -> impl Iterator<Item = (usize, &SpectrumStage)> {
        self.active
            .get(..self.len)
            .into_iter()
            .flatten()
            .filter_map(|slot| self.slots.get(*slot).map(|stage| (*slot, stage)))
    }
    #[cfg(test)]
    pub(super) fn get(&self, slot: usize) -> Option<&SpectrumStage> {
        self.slots.get(slot)
    }
    pub(super) fn observing(&self) -> bool {
        self.iter().any(|(_, stage)| stage.valid)
    }
    pub(super) fn observe_signal(
        &mut self,
        targets: Option<&SpectrumTargets>,
        identity: PostNodeIdentity,
        phase: hex_engine::NodeSignalPhase,
        offset: usize,
        frames: &[[f32; 2]],
    ) {
        let Some((slot, target)) = targets.and_then(|targets| targets.find(identity)) else {
            return;
        };
        if let Some(stage) = self
            .slots
            .get_mut(slot)
            .filter(|stage| stage.valid && stage.target == Some(target))
        {
            stage.observe_signal(identity, phase, offset, frames);
        }
    }
    pub(super) fn clear(&mut self) {
        for slot in self.active.get(..self.len).into_iter().flatten() {
            if let Some(stage) = self.slots.get_mut(*slot) {
                stage.clear();
            }
        }
        self.len = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_chain_capture_routes_exact_identity_and_has_zero_total_and_net_callback_allocations() {
        let (mut control, mut callback) = spectrum_target::channel();
        let requests: Vec<_> = (1..=MAX_SPECTRUM_TARGETS as u64)
            .map(|id| spectrum_target::SpectrumTargetRequest {
                identity: PostNodeIdentity::new(
                    hex_engine::ProcessorChainOwner::Track(1),
                    id,
                    hex_engine::DeviceTypeId::from_static("hex.eq"),
                ),
                project_generation: 7,
                activation_floor: 1,
                sample_rate_hz: 48_000,
            })
            .collect();
        control.set_targets(&requests).unwrap();
        callback.begin_block();
        let targets = callback.current_targets().unwrap();
        let mut stages = SpectrumStages::new().unwrap();
        let input = [[0.25; 2]; 256];
        let output = [[0.125; 2]; 256];
        let mut capture = || {
            stages.begin(Some(targets), callback.current_epoch(), 256, |_| true);
            for (_, target) in targets.iter() {
                stages.observe_signal(
                    Some(targets),
                    target.identity,
                    hex_engine::NodeSignalPhase::Input,
                    0,
                    &input,
                );
                stages.observe_signal(
                    Some(targets),
                    target.identity,
                    hex_engine::NodeSignalPhase::Output,
                    0,
                    &output,
                );
            }
            assert_eq!(stages.iter().count(), MAX_SPECTRUM_TARGETS);
            assert!(stages.iter().all(|(_, stage)| stage.complete()
                && stage.input_frames == input
                && stage.frames == output));
            stages.clear();
        };
        #[cfg(feature = "allocation-probe")]
        {
            let info = allocation_counter::measure(&mut capture);
            assert_eq!(
                (
                    info.count_total,
                    info.bytes_total,
                    info.count_current,
                    info.bytes_current
                ),
                (0, 0, 0, 0)
            );
        }
        #[cfg(not(feature = "allocation-probe"))]
        capture();
    }
}
