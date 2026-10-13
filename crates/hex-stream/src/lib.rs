//! Bounded verified WAV worker and borrowed project-rate page cache.
// Boxed entries keep the 4096-frame payload at a stable address while ownership
// moves through the bounded worker, ready and return queues.
#![allow(clippy::vec_box)]

mod construction;
mod construction_report;
mod construction_transport;
mod endpoint;
mod footprint;
mod page_endpoint;
mod revision;
mod runtime;
mod staging;
mod types;
mod worker;

pub use construction::{
    ConstructionCapacityReport, ConstructionCleanup, ConstructionRefusal, ConstructionState,
    FinishedStreamInput, RecoveredStreamInput, StreamConstructionRequest,
};
pub use construction_report::WorkerConstructionObservation;
pub use construction_transport::TransportSetupObservation;
pub use endpoint::EndpointStats;
pub use footprint::{
    AllocationCapacity, CatalogAllocationFootprint, ConstructionAllocationFootprint,
    EndpointAllocationFootprint, StreamAllocationFootprint, TransportAllocationFootprint,
};
pub use page_endpoint::{PageEndpointMismatch, VerifiedPageEndpoint};
pub use revision::{StreamRevisionMismatch, VerifiedStreamRevision};
pub use runtime::{
    CatalogLease, PagePoolReport, ShutdownError, StreamBusy, StreamCallbackPages, StreamControl,
    StreamPairMismatch, StreamRuntime,
};
pub use staging::{
    CompactCopyError, CompactCopyReport, CompactCopyReservations, CompactSourceInput,
    InputAllocationFootprint,
};
pub use types::{
    CatalogFootprint, PageKey, SourceDefinition, StreamConfig, StreamFailure, StreamFailureKind,
    StreamRuntimeError,
};
