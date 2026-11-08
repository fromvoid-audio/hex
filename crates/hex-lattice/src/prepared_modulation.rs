//! Immutable control-prepared modulation data, separate from active source state.
//! A graph prepares a fresh runtime before admission, then inherits committed
//! phase through its existing replacement transaction. Cloning this declaration
//! never copies a speculative audio transaction or grants publication custody.
use crate::{ModulationLfoBank, ModulationRuntime, PreparedLfoSource, PreparedModulation};
use hex_lattice_contract::DeviceContractError as Error;

/// Per-device immutable declaration limit; runtime storage is charged separately.
pub const MAX_PREPARED_MODULATION_BYTES: usize = 1_048_576;

#[derive(Clone, Debug)]
pub struct PreparedModulationState {
    sources: Vec<PreparedLfoSource>,
    program: PreparedModulation,
    chunk_frames: usize,
}

impl PreparedModulationState {
    /// Route source indices refer to the sorted source identity inventory.
    /// The host still validates device ownership and real target capabilities.
    pub fn new(
        sources: Vec<PreparedLfoSource>,
        program: PreparedModulation,
        chunk_frames: usize,
    ) -> Result<Self, Error> {
        let prepared = Self {
            sources,
            program,
            chunk_frames,
        };
        if prepared
            .storage_bytes()
            .is_none_or(|bytes| bytes > MAX_PREPARED_MODULATION_BYTES)
        {
            return Err(Error::RuntimeBudgetExceeded);
        }
        let bank = ModulationLfoBank::new(&prepared.sources, chunk_frames)?;
        if bank.source_count() != prepared.program.source_count() {
            return Err(Error::InvalidDescriptor);
        }
        Ok(prepared)
    }

    /// Construct separately owned runtime storage on control, before publication.
    pub fn prepare(&self) -> Result<ModulationRuntime, Error> {
        ModulationRuntime::new(
            ModulationLfoBank::new(&self.sources, self.chunk_frames)?,
            self.program.try_clone()?,
        )
    }

    pub fn program(&self) -> &PreparedModulation {
        &self.program
    }

    /// Owned declaration storage, including the inline header. Runtime buffers
    /// are a separate owner and must be charged using their actual capacities.
    pub fn storage_bytes(&self) -> Option<usize> {
        std::mem::size_of::<Self>()
            .checked_add(
                self.sources
                    .capacity()
                    .checked_mul(std::mem::size_of::<PreparedLfoSource>())?,
            )?
            .checked_add(self.program.storage_bytes()?)
    }
}
