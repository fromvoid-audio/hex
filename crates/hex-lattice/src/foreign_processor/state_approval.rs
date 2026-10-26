//! Immutable captured configuration matching, never native resource custody.
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, ForeignProcessorBinding, MAX_NATIVE_DEVICE_STATE_BYTES,
    NativeDeviceState,
};
use std::{any::Any, sync::Weak};

/// A weak comparison anchor supplied by the actual host source. It cannot keep
/// a processor, endpoint or joining worker alive. Hosts must keep this anchor
/// private: an arbitrary new anchor cannot match an existing original source.
pub type ForeignStateSource = Weak<dyn Any + Send + Sync>;

/// Exact asset-free processing bytes approved by their original source owner.
/// Construction is a host obligation, after an actual matching capture; this
/// type does not authenticate caller-supplied bytes. Native hosts compare its
/// private anchor to the actual original endpoint as well as checking bytes.
///
/// The fixed copied payload has no nested state allocation. A closed graph
/// constructor may clone this value into its own declared allocation without
/// adopting a provider Arc. The one weak alias is a separate handle obligation;
/// its source remains owned/accounted by the host. Graph retirement drops the
/// immutable value on control, through the existing graph disposal owner.
#[derive(Clone)]
pub struct ForeignStateApproval {
    source: ForeignStateSource,
    binding: ForeignProcessorBinding,
    device_type: DeviceTypeId,
    version: u32,
    len: usize,
    bytes: [u8; MAX_NATIVE_DEVICE_STATE_BYTES],
}
impl ForeignStateApproval {
    pub fn new_on_control(
        source: ForeignStateSource,
        binding: ForeignProcessorBinding,
        state: &NativeDeviceState,
    ) -> Result<Self, DeviceContractError> {
        state.require_asset_free_processing()?;
        if !binding.is_valid() || source.strong_count() == 0 {
            return Err(DeviceContractError::ForeignProcessingUnavailable);
        }
        let mut bytes = [0; MAX_NATIVE_DEVICE_STATE_BYTES];
        bytes
            .get_mut(..state.payload().len())
            .ok_or(DeviceContractError::StateTooLarge)?
            .copy_from_slice(state.payload());
        Ok(Self {
            source,
            binding,
            device_type: state.type_id(),
            version: state.state_version(),
            len: state.payload().len(),
            bytes,
        })
    }
    pub fn binding(&self) -> ForeignProcessorBinding {
        self.binding
    }
    /// Retain configuration DATA after the host has obtained actual source
    /// quiescence and returned its endpoint. Removing the old weak anchor lets
    /// that completed worker allocation be reclaimed. This value cannot match
    /// any live original endpoint and grants no resume or publication authority.
    pub fn detached_after_quiescence_on_control(mut self) -> Self {
        let empty: Weak<()> = Weak::new();
        self.source = empty;
        self
    }
    /// Project retained DATA onto the same actual source after its real resume
    /// acknowledgement. The host must retain original lease custody across the
    /// transition; monotonic scalar bindings alone cannot establish that fact.
    /// No plugin state is restored or recaptured by this operation.
    pub fn after_original_resume_on_control(
        &self,
        source: ForeignStateSource,
        binding: ForeignProcessorBinding,
    ) -> Result<Self, DeviceContractError> {
        let empty: Weak<()> = Weak::new();
        let empty: ForeignStateSource = empty;
        if !Weak::ptr_eq(&self.source, &empty)
            || !binding.is_valid()
            || binding.root != self.binding.root
            || binding.instance != self.binding.instance
            || binding.graph < self.binding.graph
            || binding.epoch <= self.binding.epoch
            || source.strong_count() == 0
        {
            return Err(DeviceContractError::ForeignProcessingUnavailable);
        }
        let mut resumed = self.clone();
        resumed.source = source;
        resumed.binding = binding;
        Ok(resumed)
    }
    /// Bounded byte comparison only. Independent host gains are intentionally
    /// excluded, as with NativeDeviceState::processing_eq.
    pub fn matches_state(&self, state: &NativeDeviceState) -> bool {
        state.require_asset_free_processing().is_ok()
            && state.type_id() == self.device_type
            && state.state_version() == self.version
            && state.payload() == &self.bytes[..self.len]
    }
    pub fn matches_source(&self, source: &ForeignStateSource) -> bool {
        Weak::ptr_eq(&self.source, source)
    }
    /// A matching fence for retargeting graph views. Actual native admission,
    /// live epoch and configuration checks remain the host's responsibility.
    pub fn same_source(&self, other: &Self) -> bool {
        self.binding == other.binding && Weak::ptr_eq(&self.source, &other.source)
    }
}
