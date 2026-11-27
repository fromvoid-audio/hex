//! Safe non-Unix stub for `hex-device-abi`: POSIX `dlopen`/`dlsym`/`dlclose`
//! package loading is unavailable on non-Unix hosts and fails explicitly with
//! `AbiStatus::UnsupportedAbi`.

use crate::BorrowedAbiResource;
use hex_lattice_contract::package_abi::{
    AbiDescriptor, AbiNoteEvent, AbiParamEvent, AbiParameterDescriptor, AbiStatus,
};
use std::path::Path;

#[derive(Clone)]
pub(crate) struct RawPackageModule;

pub(crate) struct RawPackageInstance {
    sample_rate: u32,
}

impl RawPackageModule {
    pub(crate) fn load(_path: &Path) -> Result<Self, AbiStatus> {
        Err(AbiStatus::UnsupportedAbi)
    }

    pub(crate) fn descriptor(&self) -> AbiDescriptor {
        AbiDescriptor {
            struct_size: std::mem::size_of::<AbiDescriptor>() as u32,
            magic: 0,
            abi_version: 0,
            device_kind: 0,
            state_version: 0,
            ui_version: 0,
            package_major: 0,
            package_minor: 0,
            package_patch: 0,
            reserved0: 0,
            capabilities: 0,
            reported_latency_frames: 0,
            tail_frames: 0,
            parameter_count: 0,
            max_state_bytes: 0,
            type_id_len: 0,
            reserved1: 0,
            type_id: [0; 128],
        }
    }

    pub(crate) fn parameter_descriptor(
        &self,
        _index: u32,
    ) -> Result<AbiParameterDescriptor, AbiStatus> {
        Err(AbiStatus::UnsupportedAbi)
    }

    pub(crate) fn create_instance(
        &self,
        _sample_rate: u32,
    ) -> Result<RawPackageInstance, AbiStatus> {
        Err(AbiStatus::UnsupportedAbi)
    }

    pub(crate) fn active_lease_count(&self) -> usize {
        0
    }
}

impl RawPackageInstance {
    pub(crate) fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub(crate) fn set_state(&mut self, _bytes: &[u8]) -> Result<(), AbiStatus> {
        Err(AbiStatus::UnsupportedAbi)
    }

    pub(crate) fn get_state(&self, _out: &mut [u8]) -> Result<usize, AbiStatus> {
        Err(AbiStatus::UnsupportedAbi)
    }

    pub(crate) fn process(
        &mut self,
        _input: &[[f32; 2]],
        output: &mut [[f32; 2]],
        _param_events: &[AbiParamEvent],
        _note_events: &[AbiNoteEvent],
    ) -> Result<(), AbiStatus> {
        output.fill([0.0; 2]);
        Err(AbiStatus::UnsupportedAbi)
    }

    pub(crate) fn process_with_resources(
        &mut self,
        _input: &[[f32; 2]],
        output: &mut [[f32; 2]],
        _param_events: &[AbiParamEvent],
        _note_events: &[AbiNoteEvent],
        _resources: &[BorrowedAbiResource<'_>],
    ) -> Result<(), AbiStatus> {
        output.fill([0.0; 2]);
        Err(AbiStatus::UnsupportedAbi)
    }
}
