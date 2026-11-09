//! Immutable control-prepared modulation data, separate from active source state.
//! A graph prepares a fresh runtime before admission, then inherits committed
//! phase through its existing replacement transaction. Cloning this declaration
//! never copies a speculative audio transaction or grants publication custody.
use crate::{
    ModulationRuntime, ModulationSourceBank, PreparedModulation, PreparedModulationSource,
    SourceModulationRoute,
    modulation_source_routes::{self, PreparedSourceRoute},
};
use hex_lattice_contract::DeviceContractError as Error;

/// Per-device immutable declaration limit; runtime storage is charged separately.
pub const MAX_PREPARED_MODULATION_BYTES: usize = 1_048_576;

#[derive(Clone, Debug)]
pub struct PreparedModulationState {
    sources: Vec<PreparedModulationSource>,
    program: PreparedModulation,
    chunk_frames: usize,
    source_routes: Vec<PreparedSourceRoute>,
}

impl PreparedModulationState {
    /// Route source indices refer to the sorted source identity inventory.
    /// The host still validates device ownership and real target capabilities.
    pub fn new(
        sources: Vec<PreparedModulationSource>,
        program: PreparedModulation,
        chunk_frames: usize,
    ) -> Result<Self, Error> {
        Self::with_source_routes(sources, program, chunk_frames, &[])
    }

    /// Add validated normalized source dependencies to the existing device
    /// program. Route indices name the exact sorted bank supplied here.
    pub fn with_source_routes(
        sources: Vec<PreparedModulationSource>,
        program: PreparedModulation,
        chunk_frames: usize,
        routes: &[SourceModulationRoute],
    ) -> Result<Self, Error> {
        ModulationSourceBank::validate_sources(&sources, chunk_frames)?;
        if sources.len() != program.source_count() {
            return Err(Error::InvalidDescriptor);
        }
        let source_routes = modulation_source_routes::prepare(&sources, routes)?;
        let prepared = Self {
            sources,
            program,
            chunk_frames,
            source_routes,
        };
        if prepared
            .storage_bytes()
            .is_none_or(|bytes| bytes > MAX_PREPARED_MODULATION_BYTES)
        {
            return Err(Error::RuntimeBudgetExceeded);
        }
        Ok(prepared)
    }

    pub(crate) fn validate_sample_rate(&self, sample_rate: u32) -> Result<(), Error> {
        if sample_rate == 0
            || self
                .sources
                .iter()
                .any(|source| source.sample_rate() != f64::from(sample_rate))
        {
            return Err(Error::InvalidParameterValue);
        }
        Ok(())
    }

    /// Construct separately owned runtime storage on control, before publication.
    pub fn prepare(&self) -> Result<ModulationRuntime, Error> {
        let mut source_routes = Vec::new();
        source_routes
            .try_reserve_exact(self.source_routes.len())
            .map_err(|_| Error::RuntimeAllocation)?;
        source_routes.extend_from_slice(&self.source_routes);
        ModulationRuntime::with_prepared_source_routes(
            ModulationSourceBank::new(&self.sources, self.chunk_frames)?,
            self.program.try_clone()?,
            source_routes,
        )
    }

    pub fn program(&self) -> &PreparedModulation {
        &self.program
    }

    /// Requested runtime capacity before allocation; graph admission must also
    /// verify the actual capacities after construction.
    pub fn quoted_runtime_heap_bytes(&self) -> Option<usize> {
        ModulationSourceBank::quoted_heap_bytes(self.sources.len(), self.chunk_frames)?
            .checked_add(self.program.storage_bytes()?)?
            .checked_add(Self::quoted_source_route_bytes(self.source_routes.len())?)
    }

    pub(crate) fn matches_runtime(&self, runtime: &ModulationRuntime) -> bool {
        runtime.matches_prepared(
            &self.sources,
            &self.program,
            &self.source_routes,
            self.chunk_frames,
        )
    }

    /// Requested declaration backing before allocating vectors on control.
    pub fn quoted_heap_bytes(sources: usize, targets: usize, routes: usize) -> Option<usize> {
        sources
            .checked_mul(std::mem::size_of::<PreparedModulationSource>())?
            .checked_add(PreparedModulation::quoted_storage_bytes(
                targets, targets, routes,
            )?)
    }

    /// Retained storage for one source route per entry.
    pub fn quoted_source_route_bytes(routes: usize) -> Option<usize> {
        routes.checked_mul(std::mem::size_of::<PreparedSourceRoute>())
    }

    /// Extra control-side backing while ordering source routes.
    pub fn quoted_source_route_overlap(routes: usize) -> Option<usize> {
        routes.checked_mul(
            std::mem::size_of::<SourceModulationRoute>()
                + std::mem::size_of::<(usize, PreparedSourceRoute)>(),
        )
    }

    /// Actual reserved declaration backing, excluding its inline header.
    pub fn heap_storage_bytes(&self) -> Option<usize> {
        self.sources
            .capacity()
            .checked_mul(std::mem::size_of::<PreparedModulationSource>())?
            .checked_add(self.program.storage_bytes()?)?
            .checked_add(Self::quoted_source_route_bytes(
                self.source_routes.capacity(),
            )?)
    }

    /// Owned declaration storage, including the inline header. Runtime buffers
    /// are a separate owner and must be charged using their actual capacities.
    pub fn storage_bytes(&self) -> Option<usize> {
        std::mem::size_of::<Self>().checked_add(self.heap_storage_bytes()?)
    }
}
