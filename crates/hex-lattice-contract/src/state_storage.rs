//! Neutral borrows of the sealed canonical state's actual private owning fields.
//! Constructors stay in lib.rs: nonempty Vec -> Arc slice for each element
//! type, fresh sized zero-array Arc then unsize for empty payloads. No COW,
//! raw/custom allocator/Weak ingress. Any new path requires requalification.
use crate::{DeviceAssetReference, NativeDeviceState};
use std::{alloc::Layout, sync::Arc};

enum Field<'a> {
    Payload(&'a Arc<[u8]>),
    References(&'a Arc<[DeviceAssetReference]>),
}
/// Borrowed actual field observation; no allocation/provenance or peak claim.
/// Consumers must separately qualify all three canonical constructor routes.
/// No owner, pointer, mutable access or reusable address token escapes.
///
/// ```compile_fail
/// use hex_lattice_contract::NativeStateStorageObservation;
/// let raw: std::sync::Arc<[u8]> = vec![1].into();
/// let _ = NativeStateStorageObservation::from(raw);
/// ```
/// ```compile_fail
/// use hex_lattice_contract::NativeDeviceState;
/// fn escape(state: &NativeDeviceState) -> hex_lattice_contract::NativeStateStorageObservation<'static> {
///     state.payload_field_on_control()
/// }
/// ```
/// ```compile_fail
/// use hex_lattice_contract::NativeDeviceState;
/// fn replace(mut state: NativeDeviceState, other: NativeDeviceState) {
///     let view = state.payload_field_on_control();
///     state = other;
///     let _ = view.len();
/// }
/// ```
pub struct NativeStateStorageObservation<'a> {
    field: Field<'a>,
}
impl NativeStateStorageObservation<'_> {
    pub fn len(&self) -> usize {
        match self.field {
            Field::Payload(owner) => owner.len(),
            Field::References(owner) => owner.len(),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn element_layout(&self) -> Layout {
        // Compile-time exhaustive leaf audit, including empty reference lists.
        let _: fn(DeviceAssetReference) = |DeviceAssetReference { slot, asset }| {
            let _: (crate::DeviceAssetSlot, crate::DeviceAssetKey) = (slot, asset);
        };
        match self.field {
            Field::Payload(_) => Layout::new::<u8>(),
            Field::References(_) => Layout::new::<DeviceAssetReference>(),
        }
    }
    pub fn owning_counts(&self) -> (usize, usize) {
        match self.field {
            Field::Payload(owner) => (Arc::strong_count(owner), Arc::weak_count(owner)),
            Field::References(owner) => (Arc::strong_count(owner), Arc::weak_count(owner)),
        }
    }
    pub fn shares_allocation_with(&self, other: &Self) -> bool {
        match (&self.field, &other.field) {
            (Field::Payload(a), Field::Payload(b)) => Arc::ptr_eq(a, b),
            (Field::References(a), Field::References(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
    pub fn is_same_owning_field_as(&self, other: &Self) -> bool {
        match (&self.field, &other.field) {
            (Field::Payload(a), Field::Payload(b)) => std::ptr::eq(*a, *b),
            (Field::References(a), Field::References(b)) => std::ptr::eq(*a, *b),
            _ => false,
        }
    }
}
impl NativeDeviceState {
    pub fn payload_field_on_control(&self) -> NativeStateStorageObservation<'_> {
        NativeStateStorageObservation {
            field: Field::Payload(&self.payload),
        }
    }
    pub fn asset_reference_field_on_control(&self) -> Option<NativeStateStorageObservation<'_>> {
        self.asset_references
            .as_ref()
            .map(|owner| NativeStateStorageObservation {
                field: Field::References(owner),
            })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DeviceAssetKey, DeviceAssetSlot, DeviceTypeId};
    fn state(bytes: Vec<u8>) -> NativeDeviceState {
        NativeDeviceState::new(DeviceTypeId::from_static("test.state-storage"), 1, bytes).unwrap()
    }
    #[test]
    fn actual_canonical_fields_distinguish_aliases_equal_allocations_and_replacement() {
        for len in [0, 1, 7, 8, 9, crate::MAX_NATIVE_DEVICE_STATE_BYTES] {
            let mut input = Vec::with_capacity(len + 37);
            input.resize(len, 3);
            let state = state(input);
            let equal = self::state(vec![3; len]);
            let alias = state.clone();
            let view = state.payload_field_on_control();
            assert_eq!(view.len(), len);
            assert_eq!(view.element_layout(), Layout::new::<u8>());
            assert_eq!(view.owning_counts(), (2, 0));
            assert!(view.shares_allocation_with(&alias.payload_field_on_control()));
            assert!(!view.is_same_owning_field_as(&alias.payload_field_on_control()));
            assert!(view.is_same_owning_field_as(&state.payload_field_on_control()));
            assert!(!view.shares_allocation_with(&equal.payload_field_on_control()));
            assert!(state.asset_reference_field_on_control().is_none());
        }
        let refs: Vec<_> = (1..=crate::MAX_DEVICE_ASSET_REFERENCES as u32)
            .map(|slot| DeviceAssetReference {
                slot: DeviceAssetSlot::new(slot).unwrap(),
                asset: DeviceAssetKey::new(1).unwrap(),
            })
            .collect();
        let original = state(vec![1]).with_asset_references(refs.clone()).unwrap();
        let clone = original.clone();
        let replaced = clone.with_asset_references(refs).unwrap();
        let hosted = state(vec![2]).with_host_metadata_from(&original).unwrap();
        let observation = original.asset_reference_field_on_control().unwrap();
        assert_eq!(observation.len(), crate::MAX_DEVICE_ASSET_REFERENCES);
        assert_eq!(
            observation.element_layout(),
            Layout::new::<DeviceAssetReference>()
        );
        assert_eq!(observation.owning_counts(), (2, 0));
        assert!(
            observation.shares_allocation_with(&hosted.asset_reference_field_on_control().unwrap())
        );
        assert!(
            !observation
                .shares_allocation_with(&replaced.asset_reference_field_on_control().unwrap())
        );
        assert!(
            original
                .payload_field_on_control()
                .shares_allocation_with(&replaced.payload_field_on_control())
        );
    }
}
