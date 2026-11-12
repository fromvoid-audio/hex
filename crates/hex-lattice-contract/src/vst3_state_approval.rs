//! Process-local provenance for immutable states captured by an original VST3 source.
//! The joining control owner retains the issuing authority; callback configuration
//! gets only a verifier. Neither value owns resources or grants native admission.
use crate::{DeviceContractError, NativeDeviceState, VST3_DEVICE_TYPE_ID};
use std::{
    num::NonZeroU64,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_AUTHORITY: AtomicU64 = AtomicU64::new(1);

/// Unique issuing capability. Keep it private to the original native control
/// owner; do not return it with source metadata, state or prepared graph facts.
#[derive(Debug)]
pub struct Vst3StateAuthority(NonZeroU64);

/// Nonissuing source correspondence check, safe to copy into callback metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Vst3StateVerifier(NonZeroU64);

impl Vst3StateAuthority {
    pub fn new_on_control() -> Result<Self, DeviceContractError> {
        Self::reserve(&NEXT_AUTHORITY)
    }

    fn reserve(next: &AtomicU64) -> Result<Self, DeviceContractError> {
        next.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
            (value != 0).then(|| value.checked_add(1)).flatten()
        })
        .ok()
        .and_then(NonZeroU64::new)
        .map(Self)
        .ok_or(DeviceContractError::RuntimeBudgetExceeded)
    }

    pub fn verifier(&self) -> Vst3StateVerifier {
        Vst3StateVerifier(self.0)
    }

    /// The source owner must independently validate capture identity, source
    /// epoch and capture completion before granting this correspondence marker.
    /// This does not qualify a changed processing profile for worker reuse.
    /// Reconstructing an opaque state never copies approval. Clone/history and
    /// independent host trim changes preserve the exact immutable payload.
    pub fn approve_on_control(
        &self,
        mut state: NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        if state.type_id() != VST3_DEVICE_TYPE_ID {
            return Err(DeviceContractError::WrongDeviceType);
        }
        state.require_asset_free_processing()?;
        state.source_approval = Some(self.0);
        state.processing_approval = None;
        Ok(state)
    }

    /// Qualify an isolated restoration whose processing profile matches the
    /// original worker. Only that control owner may issue this after checking
    /// actual restored quantum, latency, bus role/tail metadata and disposal.
    /// Parameter metadata and native graph admission remain separate checks.
    pub fn approve_processing_on_control(
        &self,
        state: NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let mut state = self.approve_on_control(state)?;
        state.processing_approval = Some(self.0);
        Ok(state)
    }
}

impl Vst3StateVerifier {
    pub fn approves(self, state: &NativeDeviceState) -> bool {
        state.source_approval == Some(self.0)
    }
    pub fn approves_processing(self, state: &NativeDeviceState) -> bool {
        self.approves(state) && state.processing_approval == Some(self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> NativeDeviceState {
        NativeDeviceState::new(VST3_DEVICE_TYPE_ID, 1, vec![1, 2, 3]).unwrap()
    }

    #[test]
    fn approval_survives_history_and_trims_without_changing_canonical_equality() {
        let authority = Vst3StateAuthority::new_on_control().unwrap();
        let verifier = authority.verifier();
        let original = state();
        let approved = authority.approve_on_control(original.clone()).unwrap();
        assert_eq!(approved, original);
        assert!(approved.processing_eq(&original));
        assert!(approved.shares_payload_storage_with(&original));
        assert!(!verifier.approves(&original));
        assert!(verifier.approves(&approved.clone()));
        let trimmed = approved
            .clone()
            .with_input_gain_db(-6.0)
            .unwrap()
            .with_output_gain_db(3.0)
            .unwrap();
        assert!(verifier.approves(&trimmed));
        assert!(verifier.approves(&trimmed.without_host_gains()));
        assert!(verifier.approves(&approved.clone().with_host_metadata_from(&original).unwrap()));
        let reconstructed = state().with_host_metadata_from(&approved).unwrap();
        assert_eq!(reconstructed, approved);
        assert!(!verifier.approves(&reconstructed));
    }

    #[test]
    fn independent_sources_cannot_verify_or_copy_processing_approval() {
        let authority = Vst3StateAuthority::new_on_control().unwrap();
        let other = Vst3StateAuthority::new_on_control().unwrap();
        let approved = authority.approve_on_control(state()).unwrap();
        assert!(!other.verifier().approves(&approved));
        let rebound = approved.clone().with_asset_references(vec![]).unwrap();
        assert_eq!(rebound, approved);
        assert!(!authority.verifier().approves(&rebound));
        let asset_state = approved
            .clone()
            .with_asset_references(vec![crate::DeviceAssetReference {
                slot: crate::DeviceAssetSlot::new(1).unwrap(),
                asset: crate::DeviceAssetKey::new(1).unwrap(),
            }])
            .unwrap();
        assert!(!authority.verifier().approves(&asset_state));
        assert!(
            !authority.verifier().approves(
                &approved
                    .clone()
                    .with_host_metadata_from(&asset_state)
                    .unwrap()
            )
        );
        assert_eq!(
            authority.approve_on_control(asset_state),
            Err(DeviceContractError::AssetResourcesUnavailable)
        );
        let native =
            NativeDeviceState::new(crate::DeviceTypeId::from_static("test.native"), 1, vec![])
                .unwrap();
        assert_eq!(
            authority.approve_on_control(native),
            Err(DeviceContractError::WrongDeviceType)
        );
    }

    #[test]
    fn exhausted_authority_counter_refuses_without_wrapping() {
        let next = AtomicU64::new(u64::MAX - 1);
        let authority = Vst3StateAuthority::reserve(&next).unwrap();
        assert_eq!(authority.0.get(), u64::MAX - 1);
        assert!(Vst3StateAuthority::reserve(&next).is_err());
        assert_eq!(next.load(Ordering::Relaxed), u64::MAX);
        let zero = AtomicU64::new(0);
        assert!(Vst3StateAuthority::reserve(&zero).is_err());
        assert_eq!(zero.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn processing_qualification_is_distinct_and_cannot_survive_reconstruction_or_downgrade() {
        let authority = Vst3StateAuthority::new_on_control().unwrap();
        let verifier = authority.verifier();
        let captured = authority.approve_on_control(state()).unwrap();
        assert!(verifier.approves(&captured));
        assert!(!verifier.approves_processing(&captured));
        let qualified = authority
            .approve_processing_on_control(captured.clone())
            .unwrap();
        assert_eq!(qualified, captured);
        assert!(qualified.shares_payload_storage_with(&captured));
        assert!(verifier.approves_processing(&qualified.clone()));
        let trimmed = qualified
            .clone()
            .with_input_gain_db(-3.0)
            .unwrap()
            .with_output_gain_db(6.0)
            .unwrap();
        assert!(verifier.approves_processing(&trimmed.without_host_gains()));
        assert!(
            verifier.approves_processing(
                &qualified
                    .clone()
                    .with_host_metadata_from(&captured)
                    .unwrap()
            )
        );
        assert!(
            !verifier
                .approves_processing(&authority.approve_on_control(qualified.clone()).unwrap())
        );
        assert!(
            !verifier
                .approves_processing(&qualified.clone().with_asset_references(vec![]).unwrap())
        );
        assert!(
            !verifier.approves_processing(&state().with_host_metadata_from(&qualified).unwrap())
        );
        let other = Vst3StateAuthority::new_on_control().unwrap();
        assert!(!other.verifier().approves_processing(&qualified));
    }
}
