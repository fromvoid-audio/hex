//! Transactional wet/dry transition for compatible zero-latency effect handoffs.

#[derive(Clone, Copy, Debug)]
pub(crate) struct BypassRamp {
    from: f32,
    elapsed: u32,
    total: u32,
}

impl BypassRamp {
    pub(crate) const INACTIVE: Self = Self {
        from: 0.0,
        elapsed: 0,
        total: 0,
    };

    pub(crate) fn is_active(self) -> bool {
        self.elapsed < self.total
    }

    fn current(self, target: f32) -> f32 {
        if self.is_active() {
            self.from + (target - self.from) * self.elapsed as f32 / self.total as f32
        } else {
            target
        }
    }

    pub(crate) fn inherited(old: Self, old_bypassed: bool, bypassed: bool, rate: u32) -> Self {
        if old_bypassed == bypassed {
            return old;
        }
        let from = old.current(if old_bypassed { 0.0 } else { 1.0 });
        let target = if bypassed { 0.0 } else { 1.0 };
        if from == target {
            return Self::INACTIVE;
        }
        Self {
            from,
            elapsed: 0,
            total: rate.div_ceil(200).max(1),
        }
    }

    pub(crate) fn remaining(self) -> usize {
        self.total.saturating_sub(self.elapsed) as usize
    }

    pub(crate) fn next(&mut self, bypassed: bool) -> f32 {
        self.elapsed = self.elapsed.saturating_add(1).min(self.total);
        self.current(if bypassed { 0.0 } else { 1.0 })
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        DeviceTypeId, NativeDeviceState, NativeRuntimeChain, PreparedProcessorInstance,
        ProcessorChainOwner, StereoMatrix,
    };
    use std::sync::Arc;

    fn chain(rate: u32, bypassed: bool) -> NativeRuntimeChain {
        let kind = DeviceTypeId::from_static("test.bypass.ramp");
        NativeRuntimeChain::new(vec![PreparedProcessorInstance {
            meter: Default::default(),
            owner: ProcessorChainOwner::Track(1),
            device_id: 7,
            device_type: kind,
            matrix: StereoMatrix::new([[2.0, 0.0], [0.0, 2.0]]).unwrap(),
            bypassed,
            sample_rate: rate,
            configuration: Some(Arc::new(NativeDeviceState::new(kind, 1, vec![]).unwrap())),
            definition: None,
        }])
        .unwrap()
    }

    fn switched(rate: u32, old_bypassed: bool) -> NativeRuntimeChain {
        let mut old = chain(rate, old_bypassed);
        let mut next = chain(rate, !old_bypassed);
        assert!(next.configuration_matches_range(&old, 0..1));
        next.transfer_history_from(&mut old);
        next
    }

    fn render(chain: &mut NativeRuntimeChain, count: usize, block: usize) -> Vec<[f32; 2]> {
        let mut frames = vec![[0.25; 2]; count];
        for part in frames.chunks_mut(block) {
            chain.begin_block();
            chain.process_block(part).unwrap();
            chain.commit_block();
        }
        frames
    }

    #[test]
    fn both_directions_are_smooth_and_independent_of_block_size_and_rate() {
        for rate in [8_000_u32, 44_100, 48_000, 192_000] {
            let length = rate.div_ceil(200) as usize;
            for old_bypassed in [false, true] {
                let reference = render(&mut switched(rate, old_bypassed), length + 5, 1);
                let mut previous = if old_bypassed { 0.25 } else { 0.5 };
                for frame in &reference {
                    assert!((frame[0] - previous).abs() <= 0.25 / length as f32 + 1e-6);
                    previous = frame[0];
                }
                assert_eq!(
                    reference[length - 1][0],
                    if old_bypassed { 0.5 } else { 0.25 }
                );
                for block in [7, 64, 128, 513] {
                    assert_eq!(
                        render(&mut switched(rate, old_bypassed), length + 5, block),
                        reference
                    );
                }
            }
        }
    }

    #[test]
    fn reversal_and_rollback_continue_from_the_committed_audible_mix() {
        let mut first = switched(48_000, false);
        let before = render(&mut first, 80, 64);
        let mut reverse = chain(48_000, false);
        reverse.transfer_history_from(&mut first);
        let mut speculative = [[0.25; 2]; 32];
        reverse.begin_block();
        reverse.process_block(&mut speculative).unwrap();
        reverse.rollback_block();
        let after = render(&mut reverse, 240, 7);
        assert!((after[0][0] - before[79][0]).abs() < 0.002);
        assert_eq!(after[239][0], 0.5);
        assert!(after.windows(2).all(|pair| pair[1][0] >= pair[0][0]));
    }
}
