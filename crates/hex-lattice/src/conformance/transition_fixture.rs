//! Original test fixture for the bounded live-transition admission contract
//! (Pulse #4178), never a browser device. It passes audio through unchanged
//! and models only the custody law a crossfading provider must follow:
//!
//! - A configuration (`mode`) change captures one outgoing transition of
//!   `TRANSITION_FRAMES` into fixed inline storage of `CAPACITY` slots, and
//!   keeps every still-active transition it inherits.
//! - A same-mode handoff carries the active transitions unchanged.
//! - Rendering ages every transition; `reset` clears them, and
//!   `settle_idle_transitions` completes them (counted in the [`Probe`]).
//! - When `previous` already holds `CAPACITY` live transitions, a mode change
//!   reports `transition_busy_from` instead of dropping one.
//!
//! A copy that would still have to drop an active transition records it in
//! the shared [`Probe`], so host tests can prove the host deferred instead.
use crate::{
    NativeProcessorRuntime, OutputTail, PreparedNativeProcessor, PreparedProcessorFactory,
    PreparedRuntimeInstance, ProcessorChainOwner, StereoFrame, StereoMatrix,
};
use hex_lattice_contract::{DeviceContractError, DeviceTypeId, NativeDeviceState};
use std::{
    any::Any,
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering::Relaxed},
    },
};

pub const TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("fixture.transition");
/// Live transitions one runtime holds inline.
pub const CAPACITY: usize = 2;
/// Frames each captured transition remains audible.
pub const TRANSITION_FRAMES: u32 = 64;

/// Shared observation of contract violations by any runtime of a fixture.
#[derive(Debug, Default)]
pub struct Probe {
    dropped: AtomicU32,
    settled: AtomicU32,
}
impl Probe {
    /// Active transitions a handoff had to discard because the host did not
    /// defer it. A conforming host keeps this at zero.
    pub fn dropped_transitions(&self) -> u32 {
        self.dropped.load(Relaxed)
    }
    /// Active transitions completed by idle-predecessor settlement
    /// (Pulse #4558) rather than by rendering.
    pub fn settled_transitions(&self) -> u32 {
        self.settled.load(Relaxed)
    }
}

struct Prepared {
    mode: u8,
    probe: Arc<Probe>,
}
impl PreparedNativeProcessor for Prepared {
    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_runtime_bytes(&self) -> usize {
        std::mem::size_of::<Runtime>()
    }
    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
        Box::new(Runtime {
            mode: self.mode,
            remaining: [0; CAPACITY],
            probe: Arc::clone(&self.probe),
        })
    }
    fn output_tail(&self) -> OutputTail {
        OutputTail::Finite {
            max_tail_frames: u64::from(TRANSITION_FRAMES),
        }
    }
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }
}

struct Runtime {
    mode: u8,
    remaining: [u32; CAPACITY],
    probe: Arc<Probe>,
}
impl Runtime {
    fn active(&self) -> usize {
        self.remaining.iter().filter(|frames| **frames > 0).count()
    }
}
impl NativeProcessorRuntime for Runtime {
    fn process_block(
        &mut self,
        frames: &mut [StereoFrame],
        _resources: &dyn crate::DeviceResources,
    ) {
        let rendered = u32::try_from(frames.len()).unwrap_or(u32::MAX);
        for remaining in &mut self.remaining {
            *remaining = remaining.saturating_sub(rendered);
        }
    }
    fn reset(&mut self) {
        self.remaining = [0; CAPACITY];
    }
    fn settle_idle_transitions(&mut self) {
        let active = u32::try_from(self.active()).unwrap_or(u32::MAX);
        self.probe.settled.fetch_add(active, Relaxed);
        self.remaining = [0; CAPACITY];
    }
    fn is_quiescent(&self) -> bool {
        self.active() == 0
    }
    fn can_copy_history_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
        previous.as_any().is::<Self>()
    }
    fn copy_history_from(&mut self, previous: &dyn NativeProcessorRuntime) {
        let Some(previous) = previous.as_any().downcast_ref::<Self>() else {
            return;
        };
        self.remaining = previous.remaining;
        if previous.mode != self.mode {
            if let Some(slot) = self.remaining.iter_mut().find(|frames| **frames == 0) {
                *slot = TRANSITION_FRAMES;
            } else {
                // Contract violation: the host admitted a busy handoff.
                self.probe.dropped.fetch_add(1, Relaxed);
            }
        }
    }
    fn can_retarget_parameters_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
        previous.as_any().is::<Self>()
    }
    fn transition_busy_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|previous| previous.mode != self.mode && previous.active() == CAPACITY)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Saved-state stand-in carrying only the fixture mode.
pub fn state(mode: u8) -> Result<NativeDeviceState, DeviceContractError> {
    NativeDeviceState::new(TYPE_ID, 1, vec![mode])
}

/// One prepared fixture entry for `owner`/`device_id` at `sample_rate`.
pub fn instance(
    owner: ProcessorChainOwner,
    device_id: u64,
    sample_rate: u32,
    mode: u8,
    probe: &Arc<Probe>,
) -> Result<PreparedRuntimeInstance, DeviceContractError> {
    Ok(PreparedRuntimeInstance {
        automation: Default::default(),
        meter: Default::default(),
        owner,
        device_id,
        device_type: TYPE_ID,
        matrix: StereoMatrix::identity(),
        bypassed: false,
        sample_rate,
        configuration: Some(Arc::new(state(mode)?)),
        definition: Some(PreparedProcessorFactory::Legacy(Arc::new(Prepared {
            mode,
            probe: Arc::clone(probe),
        }))),
    })
}

#[cfg(test)]
mod tests {
    use super::{CAPACITY, Probe, TRANSITION_FRAMES, instance};
    use crate::{NativeRuntimeChain, ProcessorChainOwner};
    use std::sync::Arc;

    fn chain(device_id: u64, mode: u8, probe: &Arc<Probe>) -> NativeRuntimeChain {
        NativeRuntimeChain::new_prepared(vec![
            instance(
                ProcessorChainOwner::Track(1),
                device_id,
                48_000,
                mode,
                probe,
            )
            .unwrap(),
        ])
        .unwrap()
    }

    fn render(chain: &mut NativeRuntimeChain, frames: usize) {
        let mut block = vec![[0.25, -0.25]; frames];
        chain.begin_block();
        chain.process_block(&mut block).unwrap();
        chain.commit_block();
    }

    /// Hand off through alternating modes until the committed runtime holds
    /// every inline transition slot. Returns the chain and its mode.
    fn saturated(probe: &Arc<Probe>) -> (NativeRuntimeChain, u8) {
        let mut previous = chain(7, 0, probe);
        let mut mode = 0;
        for _ in 0..CAPACITY {
            mode = 1 - mode;
            let mut next = chain(7, mode, probe);
            assert!(!next.transition_busy_from(&previous));
            assert!(next.can_inherit_from(&previous));
            next.inherit_from(&previous);
            previous = next;
        }
        (previous, mode)
    }

    #[test]
    fn busy_only_while_capacity_is_full_for_a_new_capture_of_the_same_identity() {
        let probe = Arc::new(Probe::default());
        let (mut full, mode) = saturated(&probe);
        let switch = 1 - mode;
        // A further mode switch must defer rather than drop a live branch.
        assert!(chain(7, switch, &probe).transition_busy_from(&full));
        // Same-mode revisions carry transitions without a capture.
        assert!(!chain(7, mode, &probe).transition_busy_from(&full));
        // Another identity never inherits this history.
        assert!(!chain(8, switch, &probe).transition_busy_from(&full));
        // Rendering past the oldest transition releases admission.
        render(&mut full, TRANSITION_FRAMES as usize - 1);
        assert!(chain(7, switch, &probe).transition_busy_from(&full));
        render(&mut full, 1);
        let mut next = chain(7, switch, &probe);
        assert!(!next.transition_busy_from(&full));
        next.inherit_from(&full);
        assert_eq!(probe.dropped_transitions(), 0);
    }

    #[test]
    fn reset_releases_admission_and_an_ignored_busy_result_drops_a_branch() {
        let probe = Arc::new(Probe::default());
        let (mut full, mode) = saturated(&probe);
        full.reset();
        assert!(!chain(7, 1 - mode, &probe).transition_busy_from(&full));

        // Why hosts must consult the query: a history copy cannot report the
        // refusal, so admitting a busy handoff discards an audible branch.
        let (full, mode) = saturated(&probe);
        let mut next = chain(7, 1 - mode, &probe);
        assert!(next.can_inherit_from(&full));
        next.inherit_from(&full);
        assert!(probe.dropped_transitions() > 0);
    }
}
