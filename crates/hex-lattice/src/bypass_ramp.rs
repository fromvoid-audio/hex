//! Transactional wet/dry transition for compatible native effect handoffs.

/// The small fixed-latency admission bound. Longer lookahead requires a
/// separately budgeted history owner.
const MAX_ALIGNED_DRY_FRAMES: usize = 16;

/// Inline host history for untrimmed dry input, in the same frame domain as
/// the processor. The chain keeps separate committed and speculative copies.
#[derive(Clone, Copy)]
pub(crate) struct AlignedDry {
    frames: [[f32; 2]; MAX_ALIGNED_DRY_FRAMES],
    len: usize,
    next: usize,
}

impl AlignedDry {
    pub(crate) fn new(latency: Option<u32>) -> Option<Self> {
        let len = usize::try_from(latency?).ok()?;
        (1..=MAX_ALIGNED_DRY_FRAMES).contains(&len).then_some(Self {
            frames: [[0.0; 2]; MAX_ALIGNED_DRY_FRAMES],
            len,
            next: 0,
        })
    }

    pub(crate) fn advance(&mut self, input: [f32; 2]) -> [f32; 2] {
        let dry = std::mem::replace(&mut self.frames[self.next], input);
        self.next += 1;
        if self.next == self.len {
            self.next = 0;
        }
        dry
    }

    pub(crate) fn is_quiet(self) -> bool {
        self.frames[..self.len]
            .iter()
            .all(|frame| *frame == [0.0; 2])
    }

    pub(crate) fn reset(&mut self) {
        self.frames.fill([0.0; 2]);
        self.next = 0;
    }
}

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

    struct AlignedFixture;
    impl crate::PreparedNativeProcessor for AlignedFixture {
        fn supports_latency_aligned_bypass(&self) -> bool {
            true
        }

        fn runtime_version(&self) -> u32 {
            1
        }

        fn declared_runtime_bytes(&self) -> usize {
            std::mem::size_of::<AlignedFixtureRuntime>()
        }

        fn create_runtime(&self) -> Box<dyn crate::NativeProcessorRuntime> {
            Box::new(AlignedFixtureRuntime(
                super::AlignedDry::new(Some(4)).unwrap(),
            ))
        }

        fn algorithmic_latency_frames(&self) -> Option<u32> {
            Some(4)
        }
    }

    #[derive(Clone, Copy)]
    struct AlignedFixtureRuntime(super::AlignedDry);
    impl crate::NativeProcessorRuntime for AlignedFixtureRuntime {
        fn process_block(
            &mut self,
            frames: &mut [crate::StereoFrame],
            _resources: &dyn crate::DeviceResources,
        ) {
            for frame in frames {
                *frame = self.0.advance(*frame).map(|sample| sample * 2.0);
            }
        }

        fn reset(&mut self) {
            self.0.reset();
        }

        fn is_quiescent(&self) -> bool {
            self.0.is_quiet()
        }

        fn can_copy_history_from(&self, previous: &dyn crate::NativeProcessorRuntime) -> bool {
            previous.as_any().is::<Self>()
        }

        fn copy_history_from(&mut self, previous: &dyn crate::NativeProcessorRuntime) {
            if let Some(previous) = previous.as_any().downcast_ref::<Self>() {
                *self = *previous;
            }
        }

        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    fn aligned_chain(bypassed: bool) -> NativeRuntimeChain {
        let kind = DeviceTypeId::from_static("test.bypass.aligned");
        let prepared = PreparedProcessorInstance {
            meter: Default::default(),
            owner: ProcessorChainOwner::Track(1),
            device_id: 7,
            device_type: kind,
            matrix: StereoMatrix::identity(),
            bypassed,
            sample_rate: 8_000,
            configuration: Some(Arc::new(NativeDeviceState::new(kind, 1, vec![]).unwrap())),
            definition: Some(Arc::new(AlignedFixture)),
        };
        assert_eq!(prepared.algorithmic_latency_frames(), Some(4));
        let chain = NativeRuntimeChain::new(vec![prepared]).unwrap();
        assert_eq!(
            chain.algorithmic_latency_frames_for_entry(0).unwrap(),
            Some(4)
        );
        chain
    }

    #[test]
    fn aligned_bypass_preserves_delay_and_rollback_through_rapid_reversal() {
        let samples: Vec<_> = (0..200).map(|n| [n as f32 / 256.0; 2]).collect();
        let mut output = samples.clone();
        let mut current = aligned_chain(false);
        let switches = [
            (16, 25, true, 1.0),
            (25, 91, false, 0.775),
            (91, 200, true, 1.0),
        ];
        current.begin_block();
        current.process_block(&mut output[..16]).unwrap();
        current.commit_block();
        for (start, end, bypassed, from) in switches {
            let mut next = aligned_chain(bypassed);
            assert!(next.configuration_matches_range(&current, 0..1));
            assert!(next.can_inherit_from(&current));
            next.transfer_history_from(&mut current);
            next.begin_block();
            next.process_block(&mut [[-0.25; 2]; 11]).unwrap();
            next.rollback_block();

            for part in output[start..end].chunks_mut(53) {
                next.begin_block();
                next.process_block(part).unwrap();
                next.commit_block();
            }
            let target = if bypassed { 0.0 } else { 1.0 };
            for (offset, actual) in output[start..end].iter().enumerate() {
                let wet = from + (target - from) * (offset + 1).min(40) as f32 / 40.0;
                let expected = samples[start + offset - 4][0] * (1.0 + wet);
                assert!(
                    (actual[0] - expected).abs() < 1e-6,
                    "frame {}",
                    start + offset
                );
                assert_eq!(actual[0], actual[1]);
            }
            current = next;
        }
    }
}
