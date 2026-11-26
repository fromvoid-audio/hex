//! Narrowly audited external C ABI loader boundary for admitted Hex-native
//! device packages (`#2990`, `#2992`).
//!
//! Excluded from the main Hex workspace so workspace `unsafe_code = "forbid"`
//! remains intact. All `unsafe` `dlopen`/`dlsym`/`dlclose` and C vtable calls
//! are isolated in private `mod raw`; this public boundary exposes safe,
//! immutable `Arc`-leased module handles and zero-allocation callback processing.
#![deny(unsafe_code)]

mod raw;

use hex_lattice_contract::package_abi::{
    AbiDescriptor, AbiNoteEvent, AbiParamEvent, AbiParameterDescriptor, AbiStatus,
};
use std::path::Path;

/// Immutable, `Arc`-leased native device package module handle.
///
/// Cloning shares the exact loaded generation lease; `dlclose` runs on control
/// only after every `PackageInstance` and `PackageModule` clone has dropped.
#[derive(Clone)]
pub struct PackageModule(raw::RawPackageModule);

/// Live C ABI device instance retaining an immutable lease to its `PackageModule`.
pub struct PackageInstance(raw::RawPackageInstance);

impl PackageModule {
    /// Load and negotiate an admitted native package dynamic library on control.
    /// Never call on an audio callback thread.
    pub fn load(path: &Path) -> Result<Self, AbiStatus> {
        raw::RawPackageModule::load(path).map(Self)
    }

    pub fn descriptor(&self) -> AbiDescriptor {
        self.0.descriptor()
    }

    pub fn parameter_descriptor(&self, index: u32) -> Result<AbiParameterDescriptor, AbiStatus> {
        self.0.parameter_descriptor(index)
    }

    /// Construct a fresh device instance on control at `sample_rate`.
    pub fn create_instance(&self, sample_rate: u32) -> Result<PackageInstance, AbiStatus> {
        self.0.create_instance(sample_rate).map(PackageInstance)
    }

    /// Current active strong lease count (module handles + live instances).
    pub fn active_lease_count(&self) -> usize {
        self.0.active_lease_count()
    }
}

impl PackageInstance {
    pub fn sample_rate(&self) -> u32 {
        self.0.sample_rate()
    }

    pub fn set_state(&mut self, bytes: &[u8]) -> Result<(), AbiStatus> {
        self.0.set_state(bytes)
    }

    pub fn get_state(&self, out: &mut [u8]) -> Result<usize, AbiStatus> {
        self.0.get_state(out)
    }

    /// Process one bounded stereo block (`<= PACKAGE_ABI_MAX_FRAMES`) with zero
    /// host heap allocation. Validates finiteness and silences output on fault.
    pub fn process(
        &mut self,
        input: &[[f32; 2]],
        output: &mut [[f32; 2]],
        param_events: &[AbiParamEvent],
        note_events: &[AbiNoteEvent],
    ) -> Result<(), AbiStatus> {
        self.0.process(input, output, param_events, note_events)
    }
}
