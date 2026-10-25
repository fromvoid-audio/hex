use super::*;
use hex_lattice::{
    NativeProcessorRuntime, NativeRuntimeChain, OutputTail, PreparedNativeProcessor,
    PreparedProcessorInstance, ProcessorChainOwner, StereoMatrix,
};
use hex_lattice_contract::DeviceTypeId;
use std::any::Any;

struct FiniteHold(u64);
#[derive(Clone)]
struct HoldRuntime {
    remaining: u64,
    bound: u64,
    held: [f32; 2],
}
impl PreparedNativeProcessor for FiniteHold {
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }

    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_runtime_bytes(&self) -> usize {
        std::mem::size_of::<HoldRuntime>()
    }
    fn output_tail(&self) -> OutputTail {
        OutputTail::Finite {
            max_tail_frames: self.0,
        }
    }
    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
        Box::new(HoldRuntime {
            remaining: 0,
            bound: self.0,
            held: [0.0; 2],
        })
    }
}
impl NativeProcessorRuntime for HoldRuntime {
    fn process_block(
        &mut self,
        frames: &mut [[f32; 2]],
        _resources: &dyn hex_lattice::DeviceResources,
    ) {
        for frame in frames {
            if *frame != [0.0; 2] {
                self.held = *frame;
                self.remaining = self.bound;
            } else if self.remaining > 0 {
                *frame = self.held;
                self.remaining -= 1;
            }
        }
    }
    fn reset(&mut self) {
        self.remaining = 0;
        self.held = [0.0; 2];
    }
    fn is_quiescent(&self) -> bool {
        self.remaining == 0
    }
    fn can_copy_history_from(&self, old: &dyn NativeProcessorRuntime) -> bool {
        old.as_any().is::<Self>()
    }
    fn copy_history_from(&mut self, old: &dyn NativeProcessorRuntime) {
        if let Some(old) = old.as_any().downcast_ref::<Self>() {
            self.remaining = old.remaining;
            self.held = old.held;
        }
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
fn chain(entries: usize, bounds: &[u64]) -> NativeRuntimeChain {
    let mut chain = NativeRuntimeChain::new(
        (0..entries)
            .map(|index| PreparedProcessorInstance {
                meter: Default::default(),
                owner: ProcessorChainOwner::Track(1),
                device_id: index as u64 + 1,
                device_type: DeviceTypeId::from_static("test.finite-hold"),
                matrix: StereoMatrix::identity(),
                bypassed: false,
                sample_rate: 48_000,
                configuration: None,
                definition: bounds
                    .get(index)
                    .map(|bound| Arc::new(FiniteHold(*bound)) as Arc<dyn PreparedNativeProcessor>),
            })
            .collect(),
    )
    .unwrap();
    chain.begin_block();
    chain.process_block(&mut [[0.001, -0.001]]).unwrap();
    chain.commit_block();
    chain
}
fn playback() -> SessionPlayback {
    let project = super::super::tests::project(0.5, 0, 8 << 32);
    let revision = super::super::tests::revision(&project, Arc::new(hex_devices::SineTable::new()));
    let mut playback = SessionPlayback::new(revision).unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    playback
}
fn seed(playback: &mut SessionPlayback, slot: usize, entries: usize, bounds: &[u64]) {
    playback.tail_groups[slot] = Some(chain(entries, bounds));
    playback.tail_output_active[slot] = true;
    playback.tail_suffix_starts[slot] = 0;
    playback.tail_mixes[slot] = crate::PreparedTrackMix::identity();
}

#[test]
fn finite_tail_horizon_and_overflow_refuse_without_moving_live_owners() {
    for (bounds, accepted, error) in [
        (
            vec![28_000_000, 800_000],
            true,
            SessionPlaybackError::ReleaseBankFull,
        ),
        (
            vec![28_000_000, 800_001],
            false,
            SessionPlaybackError::ReleaseBankFull,
        ),
        (
            vec![u64::MAX, 1],
            false,
            SessionPlaybackError::DeviceRuntime,
        ),
    ] {
        let mut previous = playback();
        let mut reference = playback();
        seed(&mut previous, 0, bounds.len(), &bounds);
        seed(&mut reference, 0, bounds.len(), &bounds);
        let mut candidate = SessionPlayback::new(Arc::clone(&previous.revision)).unwrap();
        let result = candidate.try_inherit_state_from(&mut previous);
        if accepted {
            assert!(result.is_ok());
            assert!(previous.tail_groups[0].is_none());
            let mut expected = [[0.0; 2]; 17];
            let mut actual = expected;
            reference.render(&mut expected).unwrap();
            candidate.render(&mut actual).unwrap();
            assert_eq!(actual, expected);
        } else {
            assert_eq!(result, Err(error));
            assert!(previous.tail_groups[0].is_some());
            assert!(candidate.tail_groups[0].is_none());
            let mut expected = [[0.0; 2]; 17];
            let mut actual = expected;
            reference.render(&mut expected).unwrap();
            previous.render(&mut actual).unwrap();
            assert_eq!(actual, expected);
            previous.apply(SessionPlaybackCommand::Stop).unwrap();
            assert!(candidate.try_inherit_state_from(&mut previous).is_ok());
        }
    }
}

#[test]
fn retained_entry_work_cap_counts_memoryless_entries_and_is_retryable() {
    for extra in [false, true] {
        let mut previous = playback();
        let mut reference = playback();
        for slot in 0..64 {
            seed(&mut previous, slot, 64, &[4]);
            seed(&mut reference, slot, 64, &[4]);
        }
        if extra {
            seed(&mut previous, 64, 1, &[4]);
            seed(&mut reference, 64, 1, &[4]);
        }
        let mut candidate = SessionPlayback::new(Arc::clone(&previous.revision)).unwrap();
        let preview = candidate.preview_stream_handoff_with_scope(
            &mut previous,
            2,
            None,
            &mut |_| Ok(()),
            true,
        );
        if extra {
            assert_eq!(preview, Err(SessionPlaybackError::ReleaseBankFull));
            assert_eq!(previous.tail_groups.iter().flatten().count(), 65);
            assert!(candidate.tail_groups.iter().all(Option::is_none));
        } else {
            assert!(preview.is_ok());
        }
        let result = candidate.try_inherit_state_from(&mut previous);
        let mut expected = [[0.0; 2]; 2];
        let mut actual = expected;
        reference.render(&mut expected).unwrap();
        if extra {
            assert_eq!(result, Err(SessionPlaybackError::ReleaseBankFull));
            assert_eq!(previous.tail_groups.iter().flatten().count(), 65);
            assert!(candidate.tail_groups.iter().all(Option::is_none));
            previous.render(&mut actual).unwrap();
            assert_eq!(actual, expected);
            let mut drain = [[0.0; 2]; 2];
            previous.render(&mut drain).unwrap();
            assert!(previous.tail_output_active.iter().all(|active| !active));
            assert!(
                candidate
                    .preview_stream_handoff_with_scope(
                        &mut previous,
                        2,
                        None,
                        &mut |_| Ok(()),
                        true,
                    )
                    .is_ok()
            );
            assert!(candidate.try_inherit_state_from(&mut previous).is_ok());
        } else {
            assert!(result.is_ok());
            assert_eq!(candidate.tail_groups.iter().flatten().count(), 64);
            candidate.render(&mut actual).unwrap();
            assert_eq!(actual, expected);
        }
    }
}
