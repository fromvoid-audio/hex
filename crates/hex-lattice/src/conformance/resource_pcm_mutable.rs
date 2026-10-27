//! Unregistered free-running PCM source proving the mutable resource ABI only.
//! This is not sampler event semantics or host resource admission.
use super::*;
use crate::{
    MutableInstrumentRuntime, NativeInstrumentEvent, NativeInstrumentRuntimeError,
    PreparedMutableInstrumentRuntime,
};
use std::any::Any;

pub struct Factory(BorrowedPcmEffect);
pub fn prepare(
    state: &NativeDeviceState,
    context: &DevicePreparationContext<'_>,
    index: usize,
) -> Result<Factory, DeviceContractError> {
    Ok(Factory(prepare_effect(state, context, index)?))
}
impl PreparedMutableInstrumentRuntime for Factory {
    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_twin_bytes(&self) -> usize {
        std::mem::size_of::<Runtime>()
    }
    fn max_events_per_block(&self) -> usize {
        1
    }
    fn max_active_occurrences(&self) -> usize {
        1
    }
    fn max_release_tail_frames(&self) -> u64 {
        self.0.key.2 as u64
    }
    fn create_runtime(&self) -> Box<dyn MutableInstrumentRuntime> {
        Box::new(Runtime(self.0.clone()))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
struct Runtime(BorrowedPcmEffect);
impl MutableInstrumentRuntime for Runtime {
    fn preflight_events(
        &self,
        events: &[NativeInstrumentEvent],
        _frames: usize,
        resources: &dyn DeviceResources,
    ) -> Result<(), NativeInstrumentRuntimeError> {
        if !events.is_empty() {
            return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
        }
        let (index, identity, frames, rate) = self.0.key;
        let valid = resources.get(index).is_some_and(|view| {
            view.identity == identity && view.pcm.frames() == frames && view.sample_rate == rate
        });
        if !valid {
            return Err(NativeInstrumentRuntimeError::ResourceUnavailable);
        }
        Ok(())
    }
    fn process_block(
        &mut self,
        _events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        resources: &dyn DeviceResources,
    ) {
        for frame in frames {
            let sample = self.0.note.sample(self.0.age, u64::MAX, resources);
            frame[0] += sample[0];
            frame[1] += sample[1];
            self.0.age = self.0.age.saturating_add(1);
        }
    }
    fn reset(&mut self) {
        self.0.age = 0;
    }
    fn is_quiescent(&self) -> bool {
        self.0.age >= self.0.key.2 as u64
    }
    fn can_copy_history_from(&self, previous: &dyn MutableInstrumentRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|old| old.0.key == self.0.key)
    }
    fn copy_history_from(&mut self, previous: &dyn MutableInstrumentRuntime) {
        if let Some(old) = previous
            .as_any()
            .downcast_ref::<Self>()
            .filter(|old| old.0.key == self.0.key)
        {
            self.0.age = old.0.age;
        }
    }
    fn fixed_storage_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        BorrowedDeviceResources, DeviceResourceIdentity, DeviceResourcePcm, DeviceResourceView,
        NativeInstrumentRuntimeLimits, NativeInstrumentRuntimePair, TuningTable,
    };
    use hex_lattice_contract::{
        DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot, DeviceTypeId,
    };
    #[test]
    fn mutable_twins_consume_actual_pcm_and_refuse_stale_resources_before_mutation() {
        let mono = [0.25, -0.5, 0.75, -1.0];
        let stereo = [[0.25, -0.75], [-0.5, 1.0], [0.75, -0.25], [-1.0, 0.5]];
        let asset = DeviceAssetKey::new(7).unwrap();
        let slot = DeviceAssetSlot::new(1).unwrap();
        let state = NativeDeviceState::new(
            DeviceTypeId::from_static("test.resource-pcm-mutable"),
            1,
            vec![],
        )
        .unwrap()
        .with_asset_references(vec![DeviceAssetReference { slot, asset }])
        .unwrap();
        let tuning = TuningTable::default();
        for pcm in [
            DeviceResourcePcm::Mono(&mono),
            DeviceResourcePcm::Stereo(&stereo),
        ] {
            let views = [DeviceResourceView {
                slot,
                identity: DeviceResourceIdentity {
                    asset,
                    content: [3; 32],
                    derivation_version: 1,
                    derivation: [5; 32],
                },
                sample_rate: 44_100,
                pcm,
            }];
            let resources = BorrowedDeviceResources(&views);
            let context = DevicePreparationContext {
                sample_rate: 48_000,
                tuning: &tuning,
                resources: &resources,
            };
            let factory = prepare(&state, &context, 0).unwrap();
            let mut pair = NativeInstrumentRuntimePair::new(
                &factory,
                NativeInstrumentRuntimeLimits {
                    max_pair_bytes: 4096,
                    max_events_per_block: 1,
                    max_active_occurrences: 1,
                    max_tail_frames: 4,
                },
            )
            .unwrap();
            let bad = [DeviceResourceView {
                identity: DeviceResourceIdentity {
                    content: [9; 32],
                    ..views[0].identity
                },
                ..views[0]
            }];
            let stale = BorrowedDeviceResources(&bad);
            assert_eq!(
                pair.preflight_event_batch(&[], 2, &stale),
                Err(NativeInstrumentRuntimeError::ResourceUnavailable)
            );
            pair.begin_block();
            let mut refused = [[2.0, 3.0]; 2];
            assert_eq!(
                pair.process_block(&[], &mut refused, &stale),
                Err(NativeInstrumentRuntimeError::ResourceUnavailable)
            );
            assert_eq!(refused, [[2.0, 3.0]; 2]);
            let mut first = [[0.0; 2]; 2];
            pair.process_block(&[], &mut first, &resources).unwrap();
            assert_eq!(first, [pcm.frame(0).unwrap(), pcm.frame(1).unwrap()]);
            pair.rollback_block();
            pair.begin_block();
            let mut retry = [[0.0; 2]; 1];
            pair.process_block(&[], &mut retry, &resources).unwrap();
            assert_eq!(retry, [pcm.frame(0).unwrap()]);
            pair.commit_block();
            pair.begin_block();
            let mut continuation = [[0.0; 2]; 3];
            pair.process_block(&[], &mut continuation, &resources)
                .unwrap();
            assert_eq!(
                continuation,
                [
                    pcm.frame(1).unwrap(),
                    pcm.frame(2).unwrap(),
                    pcm.frame(3).unwrap()
                ]
            );
            pair.commit_block();
            pair.begin_block();
            let mut exhausted = [[0.0; 2]; 1];
            pair.process_block(&[], &mut exhausted, &resources).unwrap();
            assert_eq!(exhausted, [[0.0; 2]]);
        }
    }
}
