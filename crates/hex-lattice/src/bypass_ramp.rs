//! Transactional wet/dry transition for compatible native effect handoffs.
use hex_lattice_contract::DeviceContractError;

const MAX_ALIGNED_DRY_FRAMES: usize = 16;
// The fixed 1.5 ms native path at the maximum 192 kHz output rate.
const MAX_NATIVE_MAIN_PASS_DRY_FRAMES: usize = 288;
// Two 256-frame bridge quanta and the admitted seven-frame Main processor.
const MAX_MAIN_PASS_DRY_FRAMES: usize = 519;

/// Value-only aligned PCM after the original Main processor has retired.
pub type MainPassDelay = InlineDryDelay<MAX_MAIN_PASS_DRY_FRAMES>;

/// Inline aligned dry history can outlive the processor that filled it.
/// It contains only PCM and a cursor, with no owned DSP or allocation.
#[derive(Clone, Copy, Debug)]
pub struct InlineDryDelay<const CAPACITY: usize> {
    frames: [[f32; 2]; CAPACITY],
    len: usize,
    next: usize,
}

impl<const CAPACITY: usize> InlineDryDelay<CAPACITY> {
    /// Copy a committed ring without transferring its allocation or provider.
    pub fn copy_committed_ring(frames: &[[f32; 2]], next: usize) -> Option<Self> {
        let mut delay = Self {
            frames: [[0.0; 2]; CAPACITY],
            len: frames.len(),
            next,
        };
        if next >= frames.len() {
            return None;
        }
        delay
            .frames
            .get_mut(..frames.len())?
            .copy_from_slice(frames);
        Some(delay)
    }

    pub fn latency_frames(&self) -> u32 {
        self.len as u32
    }

    pub fn process(&mut self, frames: &mut [[f32; 2]]) {
        for frame in frames {
            *frame = advance(&mut self.frames[..self.len], &mut self.next, *frame);
        }
    }

    pub fn reset(&mut self) {
        self.frames[..self.len].fill([0.0; 2]);
        self.next = 0;
    }
}

/// Dry input has one control-allocated owner per committed/speculative history.
/// Short legacy delays remain inline. The separately admitted owned lookahead
/// reserves exact configured storage before publication; callback operations
/// copy samples in place and never clone, replace, grow or destroy a buffer.
pub(crate) struct AlignedDry {
    storage: DryStorage,
}
// Keep lengths and cursor construction private to this history owner.
enum DryStorage {
    Inline(InlineDryDelay<MAX_ALIGNED_DRY_FRAMES>),
    Reserved { frames: Vec<[f32; 2]>, next: usize },
}
impl AlignedDry {
    pub(crate) fn new(latency: Option<u32>) -> Option<Self> {
        let len = usize::try_from(latency?).ok()?;
        (1..=MAX_ALIGNED_DRY_FRAMES).contains(&len).then_some(Self {
            storage: DryStorage::Inline(InlineDryDelay {
                frames: [[0.0; 2]; MAX_ALIGNED_DRY_FRAMES],
                len,
                next: 0,
            }),
        })
    }
    pub(crate) fn reserved(frames: u32) -> Result<Self, DeviceContractError> {
        if !(1..=crate::MAX_RESERVED_BYPASS_FRAMES).contains(&frames) {
            return Err(DeviceContractError::InvalidDescriptor);
        }
        let len =
            usize::try_from(frames).map_err(|_| DeviceContractError::RuntimeBudgetExceeded)?;
        let mut frames = Vec::new();
        frames
            .try_reserve_exact(len)
            .map_err(|_| DeviceContractError::RuntimeAllocation)?;
        // The aggregate quote charges this exact logical capacity, excluding
        // allocator bookkeeping. Refuse an allocator's larger logical reserve.
        if frames.capacity() != len {
            return Err(DeviceContractError::RuntimeBudgetExceeded);
        }
        frames.resize(len, [0.0; 2]);
        Ok(Self {
            storage: DryStorage::Reserved { frames, next: 0 },
        })
    }
    pub(crate) fn reserved_storage(&self) -> Option<&Vec<[f32; 2]>> {
        match &self.storage {
            DryStorage::Reserved { frames, .. } => Some(frames),
            DryStorage::Inline(_) => None,
        }
    }
    fn parts(&self) -> (&[[f32; 2]], usize) {
        match &self.storage {
            DryStorage::Inline(dry) => (&dry.frames[..dry.len], dry.next),
            DryStorage::Reserved { frames, next } => (frames, *next),
        }
    }
    fn parts_mut(&mut self) -> (&mut [[f32; 2]], &mut usize) {
        match &mut self.storage {
            DryStorage::Inline(dry) => (&mut dry.frames[..dry.len], &mut dry.next),
            DryStorage::Reserved { frames, next } => (frames, next),
        }
    }
    pub(crate) fn can_copy_from(&self, previous: &Self) -> bool {
        std::mem::discriminant(&self.storage) == std::mem::discriminant(&previous.storage)
            && self.parts().0.len() == previous.parts().0.len()
    }
    pub(crate) fn copy_from(&mut self, previous: &Self) -> bool {
        if !self.can_copy_from(previous) {
            return false;
        }
        let (source, old_next) = previous.parts();
        let (destination, next) = self.parts_mut();
        destination.copy_from_slice(source);
        *next = old_next;
        true
    }
    pub(crate) fn advance(&mut self, input: [f32; 2]) -> [f32; 2] {
        let (frames, next) = self.parts_mut();
        advance(frames, next, input)
    }
    pub(crate) fn main_pass_latency_frames(&self) -> Option<u32> {
        let len = self.parts().0.len();
        (1..=MAX_NATIVE_MAIN_PASS_DRY_FRAMES)
            .contains(&len)
            .then_some(len as u32)
    }
    pub(crate) fn main_pass_delay(&self) -> Option<MainPassDelay> {
        let (frames, next) = self.parts();
        MainPassDelay::copy_committed_ring(frames, next)
    }
    pub(crate) fn is_quiet(&self) -> bool {
        self.parts().0.iter().all(|f| *f == [0.0; 2])
    }
    pub(crate) fn reset(&mut self) {
        let (frames, next) = self.parts_mut();
        frames.fill([0.0; 2]);
        *next = 0;
    }
}

fn advance(frames: &mut [[f32; 2]], next: &mut usize, input: [f32; 2]) -> [f32; 2] {
    let dry = std::mem::replace(&mut frames[*next], input);
    *next += 1;
    if *next == frames.len() {
        *next = 0;
    }
    dry
}

/// Both options have the same prepared layout within one transaction. A
/// cross-owner caller must preflight compatibility; mismatch never mutates.
pub(crate) fn copy_dry(
    destination: &mut Option<AlignedDry>,
    previous: &Option<AlignedDry>,
) -> bool {
    match (destination.as_mut(), previous.as_ref()) {
        (Some(destination), Some(previous)) => destination.copy_from(previous),
        (None, None) => true,
        _ => false,
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct BypassRamp {
    from: f32,
    elapsed: u32,
    total: u32,
    dry_lead: u32,
}

impl BypassRamp {
    pub(crate) const INACTIVE: Self = Self {
        from: 0.0,
        elapsed: 0,
        total: 0,
        dry_lead: 0,
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
            dry_lead: 0,
        }
    }

    /// A settled power-off already reset DSP. Keep the outgoing aligned dry
    /// tap until fresh wet samples arrive, then de-click over half a millisecond.
    /// The longer reversal ramp remains for a still-running wet history.
    pub(crate) fn fresh_power_on(latency_frames: u32, sample_rate: u32) -> Self {
        Self {
            from: 0.0,
            elapsed: 0,
            total: sample_rate.div_ceil(2000).max(1),
            dry_lead: latency_frames,
        }
    }

    pub(crate) fn remaining(self) -> usize {
        (self.total.saturating_sub(self.elapsed) + self.dry_lead) as usize
    }

    pub(crate) fn next(&mut self, bypassed: bool) -> f32 {
        if self.dry_lead > 0 {
            self.dry_lead -= 1;
            return self.from;
        }
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

    #[test]
    fn committed_main_pass_copy_preserves_the_cursor_and_rejects_bad_extent() {
        use super::MainPassDelay;

        for len in [512, 519] {
            let mut ring = [[0.0; 2]; 519];
            let cursor = len - 1;
            ring[cursor] = [0.25, -0.25];
            ring[0] = [0.5, -0.5];
            let ring = &ring[..len];
            let mut delay = MainPassDelay::copy_committed_ring(ring, cursor).unwrap();
            let mut frames = [[0.0; 2]; 2];
            delay.process(&mut frames);
            assert_eq!(frames, [ring[cursor], ring[0]]);
            assert!(MainPassDelay::copy_committed_ring(ring, len).is_none());
        }
        assert!(MainPassDelay::copy_committed_ring(&[], 0).is_none());
        assert!(MainPassDelay::copy_committed_ring(&[[0.0; 2]; 520], 0).is_none());
    }

    #[test]
    fn dry_lead_continues_across_an_equivalent_publication_and_can_reverse() {
        use super::BypassRamp;

        let mut first = BypassRamp::fresh_power_on(3, 8_000);
        assert_eq!(first.next(false), 0.0);
        let mut next = BypassRamp::inherited(first, false, false, 8_000);
        assert_eq!(next.remaining(), 6);
        assert_eq!(next.next(false), 0.0);
        assert_eq!(next.next(false), 0.0);
        let mut cancelled = BypassRamp::inherited(next, false, true, 8_000);
        assert!(!cancelled.is_active());
        assert_eq!(cancelled.next(true), 0.0);
        assert_eq!(next.next(false), 1.0 / 4.0);
    }

    #[test]
    fn cold_power_on_keeps_exact_latency_then_finishes_its_short_declick() {
        use super::BypassRamp;

        for rate in [8_000_u32, 44_100, 48_000, 192_000] {
            for latency in [0, 11, 2366] {
                let mut ramp = BypassRamp::fresh_power_on(latency, rate);
                for _ in 0..latency {
                    assert_eq!(ramp.next(false), 0.0);
                }
                let frames = rate.div_ceil(2000);
                for frame in 1..=frames {
                    assert_eq!(ramp.next(false), frame as f32 / frames as f32);
                }
                assert!(!ramp.is_active());
                assert_eq!(ramp.remaining(), 0);
                assert_eq!(ramp.next(false), 1.0);
            }
        }
    }

    #[test]
    fn reserved_dry_is_bounded_and_copies_only_into_existing_storage() {
        for len in [1, 16, 17, 2366, crate::MAX_RESERVED_BYPASS_FRAMES] {
            let mut committed = super::AlignedDry::reserved(len).unwrap();
            let mut scratch = super::AlignedDry::reserved(len).unwrap();
            let address = scratch.parts().0.as_ptr();
            for i in 0..len + 7 {
                let actual = committed.advance([i as f32, -(i as f32)]);
                let expected = if i < len {
                    [0.0; 2]
                } else {
                    [(i - len) as f32, -((i - len) as f32)]
                };
                assert_eq!(actual, expected);
            }
            assert!(scratch.copy_from(&committed));
            assert_eq!(scratch.parts(), committed.parts());
            assert_eq!(scratch.parts().0.as_ptr(), address);
            let before = scratch.parts().0.to_vec();
            let other = super::AlignedDry::reserved(if len == 1 { 2 } else { 1 }).unwrap();
            assert!(!scratch.copy_from(&other));
            assert_eq!(scratch.parts().0, before);
            scratch.reset();
            assert!(scratch.is_quiet());
            assert_eq!(scratch.parts().0.as_ptr(), address);
        }
        for len in [0, crate::MAX_RESERVED_BYPASS_FRAMES + 1, u32::MAX] {
            assert!(super::AlignedDry::reserved(len).is_err());
        }
        assert!(super::AlignedDry::new(Some(17)).is_none());
    }

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
    fn memoryless_removal_fades_and_preserves_the_authored_bypass() {
        const RATE: u32 = 8_000;
        const FADE: usize = 40;
        let candidate = NativeRuntimeChain::empty();
        for partition in [1, 13, 257] {
            let mut running = chain(RATE, false);
            let removal = candidate.native_removal_transition_from(&running).unwrap();
            assert!(running.begin_native_removal(removal));
            assert!(!running.native_removal_ready(removal));
            let output = render(&mut running, FADE + 1, partition);
            assert!(running.native_removal_ready(removal));
            assert_eq!(output[FADE..], [[0.25; 2]]);
            for (index, sample) in output[..FADE].iter().enumerate() {
                let wet = 1.0 - (index + 1) as f32 / FADE as f32;
                assert_eq!(*sample, [0.25 + 0.25 * wet; 2]);
            }
        }

        let mut already_dry = chain(RATE, true);
        let removal = candidate
            .native_removal_transition_from(&already_dry)
            .unwrap();
        assert!(already_dry.begin_native_removal(removal));
        assert!(already_dry.native_removal_ready(removal));
        already_dry.cancel_native_removal(removal);
        assert_eq!(render(&mut already_dry, 1, 1), [[0.25; 2]]);

        let mut bypassing = switched(RATE, false);
        let mut reference = switched(RATE, false);
        let removal = candidate
            .native_removal_transition_from(&bypassing)
            .unwrap();
        assert!(bypassing.begin_native_removal(removal));
        assert!(!bypassing.native_removal_ready(removal));
        let before_cancel = render(&mut bypassing, 11, 7);
        assert!(!bypassing.native_removal_ready(removal));
        bypassing.cancel_native_removal(removal);
        let after_cancel = render(&mut bypassing, FADE - 11, 7);
        assert!(bypassing.native_removal_ready(removal));
        let actual: Vec<_> = before_cancel.into_iter().chain(after_cancel).collect();
        assert_eq!(actual, render(&mut reference, FADE, 1));
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
                self.0.copy_from(&previous.0);
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
