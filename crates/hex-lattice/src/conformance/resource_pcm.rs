//! Synthetic borrowed-PCM ABI fixture; neither a registered sampler nor root admission.
use crate::{
    ANALYTIC_NOTE_PAYLOAD_WORDS, DevicePreparationContext, DeviceResources, PreparedAnalyticNote,
    StereoFrame,
};
use hex_lattice_contract::{DeviceContractError, NativeDeviceState};

/// Prepare scalar-only note data against a validated borrowed resource inventory.
/// The caller retains the exact immutable owner through every processing call.
/// Engine registries still refuse nonempty assets pending actual root admission.
pub fn prepare_note(
    state: &NativeDeviceState,
    context: &DevicePreparationContext<'_>,
    index: usize,
) -> Result<PreparedAnalyticNote, DeviceContractError> {
    context.validate_for(state)?;
    let view = context
        .resources
        .get(index)
        .ok_or(DeviceContractError::AssetResourcesUnavailable)?;
    let mut payload = [0; ANALYTIC_NOTE_PAYLOAD_WORDS];
    payload[0] =
        u64::try_from(index).map_err(|_| DeviceContractError::AssetResourcesUnavailable)?;
    payload[1] = view.pcm.frames() as u64;
    payload[2] = view.identity.asset.value();
    Ok(PreparedAnalyticNote::new(payload, 0, sample))
}
fn sample(
    payload: &[u64; ANALYTIC_NOTE_PAYLOAD_WORDS],
    age: u64,
    gate: u64,
    _release: u64,
    resources: &dyn DeviceResources,
) -> StereoFrame {
    if age >= gate || age >= payload[1] {
        return [0.0; 2];
    }
    let Some(view) = usize::try_from(payload[0])
        .ok()
        .and_then(|index| resources.get(index))
    else {
        return [0.0; 2];
    };
    if view.identity.asset.value() != payload[2] || view.pcm.frames() as u64 != payload[1] {
        return [0.0; 2];
    }
    usize::try_from(age)
        .ok()
        .and_then(|index| view.pcm.frame(index))
        .unwrap_or([0.0; 2])
}

/// Synthetic resource-consuming effect for callback ABI conformance. No native
/// device registration or host resource admission is provided by this fixture.
#[derive(Clone)]
pub struct BorrowedPcmEffect {
    note: PreparedAnalyticNote,
    key: (usize, crate::DeviceResourceIdentity, usize, u32),
    age: u64,
}
pub fn prepare_effect(
    state: &NativeDeviceState,
    context: &DevicePreparationContext<'_>,
    index: usize,
) -> Result<BorrowedPcmEffect, DeviceContractError> {
    let note = prepare_note(state, context, index)?;
    let view = context
        .resources
        .get(index)
        .ok_or(DeviceContractError::AssetResourcesUnavailable)?;
    Ok(BorrowedPcmEffect {
        note,
        key: (index, view.identity, view.pcm.frames(), view.sample_rate),
        age: 0,
    })
}
impl crate::NativeProcessorRuntime for BorrowedPcmEffect {
    fn process_block(&mut self, frames: &mut [StereoFrame], resources: &dyn DeviceResources) {
        for frame in frames {
            let gain = self.note.sample(self.age, u64::MAX, resources);
            frame[0] *= gain[0];
            frame[1] *= gain[1];
            self.age = self.age.saturating_add(1);
        }
    }
    fn reset(&mut self) {
        self.age = 0;
    }
    fn is_quiescent(&self) -> bool {
        self.age >= self.key.2 as u64
    }
    fn can_copy_history_from(&self, previous: &dyn crate::NativeProcessorRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|previous| self.key == previous.key)
    }
    fn copy_history_from(&mut self, previous: &dyn crate::NativeProcessorRuntime) {
        if let Some(previous) = previous
            .as_any()
            .downcast_ref::<Self>()
            .filter(|previous| self.key == previous.key)
        {
            self.age = previous.age;
        }
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[path = "resource_pcm_mutable.rs"]
pub mod mutable;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        BorrowedDeviceResources, DeviceResourceIdentity, DeviceResourcePcm, DeviceResourceView,
        TuningTable,
    };
    use hex_lattice_contract::{
        DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot, DeviceTypeId,
    };
    #[test]
    fn actual_note_callback_borrows_mono_and_stereo_without_owning_or_copying_pcm() {
        let mono = [0.25_f32, -0.5, 0.75, 0.0];
        let stereo = [[0.5_f32, -0.25], [0.0, 0.75], [-0.5, 0.0], [0.25, 0.5]];
        for pcm in [
            DeviceResourcePcm::Mono(&mono),
            DeviceResourcePcm::Stereo(&stereo),
        ] {
            let slot = DeviceAssetSlot::new(1).unwrap();
            let asset = DeviceAssetKey::new(7).unwrap();
            let state =
                NativeDeviceState::new(DeviceTypeId::from_static("test.resource-pcm"), 1, vec![])
                    .unwrap()
                    .with_asset_references(vec![DeviceAssetReference { slot, asset }])
                    .unwrap();
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
            let tuning = TuningTable::default();
            let context = DevicePreparationContext {
                sample_rate: 48_000,
                tuning: &tuning,
                resources: &resources,
            };
            let note = prepare_note(&state, &context, 0).unwrap();
            for age in 0..7 {
                let expected = if age < 3 {
                    match pcm {
                        DeviceResourcePcm::Mono(_) => [mono[age]; 2],
                        DeviceResourcePcm::Stereo(_) => stereo[age],
                    }
                } else {
                    [0.0; 2]
                };
                assert_eq!(note.sample(age as u64, 3, &resources), expected);
            }
            // Partitioning and a direct seek use authored age, with the same borrowed root.
            let mut partitioned = Vec::new();
            let mut age = 0;
            for count in [1, 2, 1] {
                for _ in 0..count {
                    partitioned.push(note.sample(age, 4, &resources));
                    age += 1;
                }
            }
            let direct: Vec<_> = (0..4).map(|age| note.sample(age, 4, &resources)).collect();
            assert_eq!(partitioned, direct);
            assert_eq!(note.sample(2, 4, &resources), direct[2]);
            // Actual effect trait callbacks consume the same borrowed PCM;
            // default bypass advances history while preserving the dry block.
            use crate::NativeProcessorRuntime;
            let mut effect = prepare_effect(&state, &context, 0).unwrap();
            let mut first = [[2.0_f32, 3.0]; 2];
            effect.process_block(&mut first, &resources);
            for (index, frame) in first.iter().enumerate() {
                let gain = pcm.frame(index).unwrap();
                assert_eq!(*frame, [2.0 * gain[0], 3.0 * gain[1]]);
            }
            effect.reset();
            let mut dry = [[2.0_f32, 3.0]; 2];
            effect.process_bypassed_block(&mut dry, &resources);
            assert_eq!(dry, [[2.0, 3.0]; 2]);
            let mut after_bypass = [[2.0_f32, 3.0]];
            effect.process_block(&mut after_bypass, &resources);
            let gain = pcm.frame(2).unwrap();
            assert_eq!(after_bypass, [[2.0 * gain[0], 3.0 * gain[1]]]);
            assert!(prepare_note(&state, &context, 1).is_err());
            let bad = [DeviceResourceView {
                identity: DeviceResourceIdentity {
                    asset: DeviceAssetKey::new(8).unwrap(),
                    ..views[0].identity
                },
                ..views[0]
            }];
            let stale = BorrowedDeviceResources(&bad);
            assert!(
                prepare_note(
                    &state,
                    &DevicePreparationContext {
                        resources: &stale,
                        ..context
                    },
                    0
                )
                .is_err()
            );
        }
    }
}
