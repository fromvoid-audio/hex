//! Bounded verified WAV worker and borrowed project-rate page cache.
// Boxed entries keep the 4096-frame payload at a stable address while ownership
// moves through the bounded worker, ready and return queues.
#![allow(clippy::vec_box)]

mod footprint;
pub use footprint::{AllocationData, EndpointData, StreamDataFootprint, TransportData, WorkerData};
mod endpoint;
mod runtime;
mod types;
mod worker;

pub use endpoint::EndpointStats;
pub use runtime::{
    AudioStreamStorageError, AudioStreamStorageLedger, AudioStreamStorageReservation,
    AudioStreamStorageSummary, CatalogLease, OpaqueStreamCost, OpaqueStreamHandleGroup,
    PagePoolReport, PreparedStreamDisposal, ShutdownError, StreamBusy, StreamCallbackPages,
    StreamControl, StreamDisposalError, StreamDisposalPhase, StreamHandleLocation,
    StreamOwningHandleObligation, StreamPairMismatch, StreamRuntime, StreamSourceDomainObligation,
};
pub use types::{
    CatalogFootprint, PageKey, SourceDefinition, StreamConfig, StreamFailure, StreamFailureKind,
    StreamRuntimeError,
};
